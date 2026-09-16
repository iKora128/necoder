//! Fleet の舞台。配置とタブの選択だけを持ち、会話・PTY の所有権は Session に残す。
use crate::workspace::*;
use super::fleet_view::pane_space;

impl Workspace {
    pub(super) fn stage_cards(&self) -> Vec<(usize, FleetPane)> {
        let active = self.active_slot().map(|slot| slot.task_space.id.clone());
        let mut spaces = Vec::new();
        if self.chrome.stage_columns > 1 {
            spaces.extend(self.chrome.stage_pinned.iter().cloned());
        }
        if let Some(active) = active {
            if !spaces.contains(&active) { spaces.push(active); }
        }
        let limit = self.chrome.stage_columns.min(((self.chrome.stage_width / 428.) as usize).max(1));
        spaces.into_iter().filter_map(|space| {
            let session = self.session_index_for_space(&space)?;
            let slot = &self.project_sessions.projects[session];
            if Some(slot.repository_key()) != self.active_repository_key() || slot.task_space.phase == TaskPhase::Archived { return None; }
            self.chrome.fleet_cells.iter().enumerate()
                .find(|(_, pane)| pane_space(pane) == &space)
                .map(|(index, _)| (index, FleetPane::Task { space }))
        }).take(limit).collect()
    }

    pub(super) fn render_stage_toolbar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut bar = div().flex_none().flex().items_center().justify_end().gap(px(5.)).px(px(10.)).h(px(30.));
        for columns in 1..=3usize {
            bar = bar.child(div().id(("stage-layout", columns)).px(px(9.)).rounded(px(4.))
                .text_size(px(11.)).text_color(self.theme.fg1)
                .when(self.chrome.stage_columns == columns, |e| e.bg(self.theme.bg2))
                .cursor_pointer().child(SharedString::from(i18n::t!("fleet.stage_columns", "count" => columns)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.chrome.stage_columns = columns;
                    cx.notify();
                })));
        }
        bar.into_any_element()
    }

    pub(super) fn toggle_stage_pin(&mut self, space: SpaceId, cx: &mut Context<Self>) {
        if let Some(index) = self.chrome.stage_pinned.iter().position(|current| current == &space) {
            self.chrome.stage_pinned.remove(index);
        } else {
            if self.chrome.stage_pinned.len() == 3 { self.chrome.stage_pinned.remove(0); }
            self.chrome.stage_pinned.push(space);
            self.chrome.stage_columns = self.chrome.stage_pinned.len().max(2);
        }
        cx.notify();
    }

    pub(super) fn render_task_tabs(&self, session_index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let session = &self.project_sessions.sessions[session_index];
        let space = self.project_sessions.projects[session_index].task_space.id.clone();
        let mut bar = div().id(("task-tabs", session_index)).flex_none().flex().overflow_x_scroll().items_center().h(px(30.)).gap(px(4.)).px(px(6.));
        let mut tabs = Vec::new();
        for panel in &session.fleet_agents {
            for (thread, status) in panel.read(cx).statuses().into_iter().enumerate() {
                tabs.push((status.name, FleetPane::Agent { space: space.clone(), panel: panel.clone() }, Some(thread)));
            }
        }
        tabs.push((i18n::t!("fleet.tab_diff").into(), FleetPane::Diff { space: space.clone() }, None));
        for pane in &self.chrome.fleet_cells {
            if let FleetPane::Shell { space: owner, id } = pane {
                if owner == &space { tabs.push((format!("{} {id}", i18n::t!("fleet.tab_terminal")).into(), pane.clone(), None)); }
            }
        }
        tabs.push((i18n::t!("fleet.tab_files").into(), FleetPane::Editor { space: space.clone() }, None));
        for (tab_index, (label, pane, thread)) in tabs.into_iter().enumerate() {
            let selected = self.chrome.stage_tabs.get(&space).is_some_and(|current| current == &pane)
                && thread.is_none_or(|thread| match &pane { FleetPane::Agent { panel, .. } => panel.read(cx).active_thread() == thread, _ => true });
            let target = space.clone();
            bar = bar.child(div().id(("task-tab", session_index * 10000 + tab_index)).flex_none().px(px(7.)).py(px(4.))
                .text_size(px(10.)).text_color(self.theme.fg1).border_b_2()
                .border_color(if selected { self.project_sessions.projects[session_index].color } else { self.theme.border })
                .cursor_pointer().child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.switch_project(session_index, window, cx);
                    if let FleetPane::Agent { panel, .. } = &pane {
                        if let Some(thread) = thread { panel.update(cx, |panel, cx| panel.focus_thread(thread, cx)); }
                        this.project_sessions.sessions[session_index].agent_panel = panel.clone();
                    }
                    if matches!(&pane, FleetPane::Diff { .. }) { this.refresh_git_status_for(session_index, cx); }
                    this.chrome.stage_tabs.insert(target.clone(), pane.clone());
                    cx.notify();
                })));
        }
        for (id, key) in [(0usize, "fleet.tab_add_thread"), (1, "fleet.tab_add_terminal")] {
            bar = bar.child(div().id(("task-tab-add", session_index * 2 + id)).flex_none().px(px(7.)).text_size(px(10.)).text_color(self.theme.fg2).cursor_pointer()
                .child(SharedString::from(i18n::t!(key)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.switch_project(session_index, window, cx);
                    if id == 0 { this.add_fleet_agent(cx); } else { this.add_terminal_to_selected_task(cx); }
                })));
        }
        bar.into_any_element()
    }

    pub(super) fn render_lineage_strip(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut strip = div().id("lineage-strip").h(px(34.)).flex_none().flex().items_center().gap(px(12.)).px(px(12.)).overflow_x_scroll();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if Some(slot.repository_key()) != self.active_repository_key() || slot.task_space.phase == TaskPhase::Archived { continue; }
            strip = strip.child(div().id(("lineage-task", index)).flex_none().text_size(px(10.)).text_color(slot.color).cursor_pointer()
                .child(SharedString::from(format!("{} {}", if slot.task_space.is_integration() { "━" } else if slot.task_space.phase == TaskPhase::Integrated { "╰━" } else { "┬━" }, slot.task_space.title)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| { this.switch_project(index, window, cx); })));
        }
        strip.into_any_element()
    }
}

impl Workspace {
    pub(super) fn render_stage_files(&self, session: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let slot = &self.project_sessions.projects[session];
        let mut tree = div().id(("stage-files", session)).size_full().overflow_y_scroll();
        for (index, row) in slot.explorer.rows.iter().enumerate() {
            let path = row.path.clone();
            let directory = row.is_dir;
            let label = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            tree = tree.child(div().id(("stage-file", index)).pl(px(12. + row.depth as f32 * 12.)).h(px(24.)).text_size(px(12.)).text_color(self.theme.fg1).cursor_pointer()
                .child(SharedString::from(format!("{} {label}", if directory { "▸" } else { "·" })))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.switch_project(session, window, cx);
                    if directory { this.toggle_dir(path.clone(), cx); } else {
                        this.chrome.fleet_mode = false;
                        this.open_file(path.clone(), window, cx);
                    }
                })));
        }
        tree.into_any_element()
    }
}

impl Workspace {
    pub(super) fn render_task_header(&self, cell: usize, space: &SpaceId, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(index) = self.session_index_for_space(space) else { return div().into_any_element(); };
        let slot = &self.project_sessions.projects[index];
        let phase = slot.task_space.phase;
        let (phase_key, action_key) = match phase {
            TaskPhase::Blocked => ("fleet.phase_blocked", "fleet.next_allow"),
            TaskPhase::Failed | TaskPhase::ChangesRequested => ("fleet.phase_failed", "fleet.next_fix"),
            TaskPhase::ReviewReady => ("fleet.phase_review", "fleet.next_review"),
            TaskPhase::MergeReady => ("fleet.phase_review", "control.integrate"),
            TaskPhase::Integrating | TaskPhase::Integrated => ("fleet.phase_integrated", "fleet.cleanup_tip"),
            _ => ("fleet.phase_working", "fleet.next_review"),
        };
        let renaming = self.chrome.task_renaming.as_ref().filter(|rename| rename.index == index && rename.site == RenameSite::Cell).map(|rename| rename.editor.clone());
        let title = if let Some(editor) = renaming { div().h(px(22.)).child(editor).into_any_element() } else {
            div().id(("task-title", index)).overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(self.theme.fg0).child(slot.task_space.title.clone())
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if event.click_count == 2 { this.start_task_rename(index, RenameSite::Cell, window, cx); }
                })).into_any_element()
        };
        div().flex_none().flex().items_center().gap(px(7.)).px(px(10.)).py(px(6.)).border_b_1().border_color(self.theme.border)
            .child(div().size(px(7.)).rounded_full().bg(slot.color))
            .child(div().flex_1().min_w_0().child(title))
            .child(div().text_size(px(10.)).text_color(self.theme.fg2).child(SharedString::from(format!("⎇ {}", slot.branch.as_deref().unwrap_or("")))))
            .child(div().text_size(px(9.)).text_color(self.theme.fg2).child(SharedString::from(i18n::t!(phase_key))))
            .when(index == self.project_sessions.active && !slot.task_space.is_integration(), |header| header.child(
                div().id(("task-next", index)).cursor_pointer().text_size(px(10.)).text_color(self.theme.fg1).child(SharedString::from(i18n::t!(action_key)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        let space = this.project_sessions.projects[index].task_space.id.clone();
                        match phase {
                            TaskPhase::MergeReady => this.integrate_task(space, cx),
                            TaskPhase::Integrated | TaskPhase::Integrating => this.open_fleet_cell_menu(cell, event.position, cx),
                            TaskPhase::Failed | TaskPhase::ChangesRequested => {
                                let panel = this.project_sessions.sessions[index].agent_panel.clone();
                                panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
                            }
                            TaskPhase::Blocked => {
                                let pending = this.project_sessions.sessions[index].agent_statuses(cx).into_iter().find_map(|(panel, thread, _)| {
                                    let card = panel.read(cx).permission_card(thread)?;
                                    let option = card.options.iter().find(|(_, kind, _)| *kind == agent_panel::AgentPermissionKind::Allow)?.0;
                                    Some((panel, thread, option))
                                });
                                if let Some((panel, thread, option)) = pending {
                                    panel.update(cx, |panel, cx| panel.respond_permission(thread, option, cx));
                                    this.transition_task_space(index, TaskPhase::Working, "permission_resolved", None, cx);
                                }
                            },
                            _ => this.review_task_for_merge(space, cx),
                        }
                    }))))
            .child(div().id(("task-expand", index)).cursor_pointer().text_color(self.theme.fg2).child("⤢")
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.chrome.stage_columns = 1; this.switch_project(index, window, cx); cx.notify();
                })))
            .child(div().id(("task-menu", index)).cursor_pointer().text_color(self.theme.fg2).child("⋯")
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| this.open_fleet_cell_menu(cell, event.position, cx))))
            .into_any_element()
    }
}
