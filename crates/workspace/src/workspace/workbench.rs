//! リポジトリの机。列は worktree、ペインは表示、セッションは既存 ProjectSession に残す。
use crate::workspace::*;

pub(crate) struct WorkResize {
    repository: String,
    first: usize,
    column: Option<usize>,
    origin: f32,
    first_weight: f32,
    second_weight: f32,
}

#[derive(Clone, Copy)]
enum WorkAction {
    Agent,
    NewThread,
    Terminal,
    DockTerminal,
    File,
    Diff,
    Split,
    Vertical,
    MoveLeft,
    MoveRight,
    MoveTabLeft,
    MoveTabRight,
    CloseTab,
    ClosePane,
    CloseColumn,
    Maximize,
    ReturnTerminal,
    StopTerminal,
}

impl Workspace {
    /// 実在する fixture worktree で入力先・セッション保持・配置を検証する。
    #[cfg(debug_assertions)]
    pub fn debug_workbench_probe(
        &mut self,
        mode: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chrome.fleet_mode = true;
        self.chrome.fleet_center_view = FleetCenterView::Work;
        self.chrome.show_settings = false;
        self.chrome.show_left = true;
        self.chrome.show_herd = true;
        self.chrome.bottom_height = BOTTOM_DOCK_MIN;
        self.chrome.work_layout = WorkLayoutState::default();
        for index in 0..self.project_sessions.projects.len().min(3) {
            self.open_work_space(index, true, window, cx);
        }
        let Some(repository) = self.work_repository() else {
            return;
        };
        let panes: Vec<_> = self.chrome.work_layout.repositories[&repository]
            .columns
            .iter()
            .flat_map(|column| column.panes.iter().map(|pane| pane.id))
            .collect();
        if let Some(&pane) = panes.get(1).or(panes.first()) {
            self.run_work_action(pane, WorkAction::Split, window, cx);
            let lower = self.chrome.work_layout.repositories[&repository]
                .focused
                .expect("split pane");
            self.run_work_action(lower, WorkAction::Terminal, window, cx);
            let (_, space) = self.work_location(lower).expect("space");
            let index = self
                .session_index_for_space(&SpaceId(space))
                .expect("session");
            let ids = self.project_sessions.sessions[index]
                .terminal_dock
                .read(cx)
                .detached_sessions();
            assert_eq!(ids.len(), 2);
            let dock = self.project_sessions.sessions[index].terminal_dock.clone();
            let first = dock.read(cx).session(ids[0]).expect("first terminal");
            let second = dock.read(cx).session(ids[1]).expect("second terminal");
            assert_ne!(first.entity_id(), second.entity_id());
            second.read(cx).insert_text("pwd\n");
            // 上下 / 横縦切替で Entity と入力先が変わらない。
            self.run_work_action(lower, WorkAction::Vertical, window, cx);
            assert_eq!(
                second.entity_id(),
                dock.read(cx)
                    .session(ids[1])
                    .expect("same terminal")
                    .entity_id()
            );
            // 下ドックへ移動し、同じ Entity をもう一度 Fleet に戻す。
            self.run_work_action(lower, WorkAction::ReturnTerminal, window, cx);
            assert_eq!(
                second.entity_id(),
                dock.read(cx).active_terminal().expect("docked").entity_id()
            );
            self.run_work_action(lower, WorkAction::DockTerminal, window, cx);
            self.chrome.bottom_height = BOTTOM_DOCK_MIN;
            let surface = self.chrome.work_layout.repositories[&repository]
                .pane(lower)
                .and_then(WorkPane::selected)
                .cloned();
            let Some(WorkSurface::Terminal { id }) = surface else {
                panic!("terminal tab");
            };
            assert_eq!(
                second.entity_id(),
                dock.read(cx).session(id).expect("moved back").entity_id()
            );
            if mode == "menu" {
                self.chrome.work_menu = Some((lower, point(px(0.), px(0.))));
            }
            if mode == "restore" {
                let state = self.chrome.work_layout.clone();
                let encoded = serde_json::to_string(&state).expect("encode");
                self.chrome.work_layout = serde_json::from_str(&encoded).expect("decode");
                self.chrome.work_layout.sanitize();
                assert_eq!(
                    second.entity_id(),
                    dock.read(cx)
                        .session(id)
                        .expect("restored placement")
                        .entity_id()
                );
            }
        }
        if mode == "vertical" {
            for layout in self.chrome.work_layout.repositories.values_mut() {
                for column in &mut layout.columns {
                    for pane in &mut column.panes {
                        pane.vertical_tabs = true;
                    }
                }
            }
        }
        eprintln!("WORKBENCH_PROBE_OK: independent terminals, dock transfer, tab orientation, saved layout");
        cx.notify();
    }

    pub(crate) fn open_worktree_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let origin = worktree.clone();
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        self.chrome.worktree_origin = Some(origin.clone());
        cx.spawn(async move |_, cx| {
            let choices = cx
                .background_executor()
                .spawn(async move {
                    let worktrees = project::git_worktrees_on(host.as_ref(), &root);
                    let branches = project::git_branches_on(host.as_ref(), &root);
                    let mut choices: Vec<_> = worktrees
                        .into_iter()
                        .map(|tree| {
                            (
                                tree.branch.unwrap_or_else(|| {
                                    tree.path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string()
                                }),
                                Some(tree.path),
                            )
                        })
                        .collect();
                    for branch in branches {
                        if !choices.iter().any(|(name, _)| name == &branch) {
                            choices.push((branch, None));
                        }
                    }
                    choices
                })
                .await;
            if let Err(error) = handle.update(cx, |workspace, window, cx| {
                if !workspace
                    .chrome
                    .worktree_origin
                    .as_ref()
                    .is_some_and(|current| Rc::ptr_eq(current, &origin))
                {
                    return;
                }
                let items = choices
                    .iter()
                    .enumerate()
                    .map(|(id, (branch, path))| {
                        PickerItem::new(id, format!("⎇ {branch}")).with_detail(
                            path.as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| i18n::t!("work.create_existing")),
                        )
                    })
                    .collect();
                workspace.chrome.worktree_choices = choices;
                workspace.open_picker(
                    PickerMode::Worktrees,
                    i18n::t!("work.choose_worktree"),
                    items,
                    window,
                    cx,
                );
                if let Some(picker) = &workspace.overlays.picker {
                    picker.update(cx, |picker, cx| {
                        picker.set_query_action(usize::MAX, i18n::t!("work.create_branch"), cx)
                    });
                }
            }) {
                eprintln!("worktree picker: {error:#}");
            }
        })
        .detach();
    }

    pub(crate) fn confirm_worktree_choice(
        &mut self,
        id: usize,
        query: String,
        cx: &mut Context<Self>,
    ) {
        let Some(origin) = self.chrome.worktree_origin.take() else {
            return;
        };
        let new_branch = id == usize::MAX;
        let choice = if new_branch {
            Some((query.trim().to_string(), None))
        } else {
            self.chrome.worktree_choices.get(id).cloned()
        };
        let Some((branch, existing)) = choice else {
            return;
        };
        if branch.is_empty() {
            return;
        }
        let host = origin.host().clone();
        let opening_host = host.clone();
        let root = origin.root().to_path_buf();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(path) = existing {
                        return Ok((path, branch));
                    }
                    // 再確認して他 UI / CLI で既に開かれた worktree を再利用する。
                    if !new_branch {
                        if let Some(tree) = project::git_worktrees_on(host.as_ref(), &root)
                            .into_iter()
                            .find(|tree| tree.branch.as_deref() == Some(&branch))
                        {
                            return Ok((tree.path, branch));
                        }
                    }
                    let parent = root
                        .parent()
                        .ok_or_else(|| anyhow::anyhow!("worktree root has no parent"))?;
                    let slug: String = branch
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == '-' || c == '_' {
                                c
                            } else {
                                '-'
                            }
                        })
                        .collect();
                    let name = root.file_name().unwrap_or_default().to_string_lossy();
                    let mut target = parent.join(format!("{name}-{slug}"));
                    let mut suffix = 2;
                    while host.metadata(&target).is_ok() {
                        target = parent.join(format!("{name}-{slug}-{suffix}"));
                        suffix += 1;
                    }
                    if new_branch {
                        project::create_task_worktree_on(host.as_ref(), &root, &target, &branch)?;
                    } else {
                        project::add_worktree_on(host.as_ref(), &root, &target, &branch)?;
                    }
                    Ok::<_, anyhow::Error>((target, branch))
                })
                .await;
            if let Err(error) = workspace.update(cx, |workspace, cx| match result {
                Ok((target, branch)) => {
                    workspace.open_folder_in_rail(
                        opening_host.clone(),
                        target.clone(),
                        Some(branch),
                        cx,
                    );
                    if workspace.chrome.fleet_mode
                        && workspace.chrome.fleet_center_view == FleetCenterView::Graph
                    {
                        if let Some(slot) =
                            workspace.project_sessions.projects.iter().find(|slot| {
                                slot.worktree.root() == target
                                    && slot.worktree.host().id() == opening_host.id()
                            })
                        {
                            let pane = FleetPane::Task {
                                space: slot.task_space.id.clone(),
                            };
                            if !workspace.chrome.fleet_cells.contains(&pane) {
                                workspace.chrome.fleet_cells.push(pane);
                            }
                            workspace.chrome.fleet_maximized = None;
                            cx.notify();
                        }
                    }
                }
                Err(error) => workspace.push_toast(
                    SharedString::from(format!("{error:#}")),
                    workspace.accent(),
                    cx,
                ),
            }) {
                eprintln!("open worktree: {error:#}");
            }
        })
        .detach();
    }

    pub(crate) fn work_repository(&self) -> Option<String> {
        self.active_slot().map(|slot| {
            if slot.task_space.repository_id.is_empty() {
                slot.task_space.id.0.clone()
            } else {
                slot.task_space.repository_id.clone()
            }
        })
    }

    pub(crate) fn work_is_visible(&self) -> bool {
        self.chrome.fleet_mode
            && self.chrome.fleet_center_view == FleetCenterView::Work
            && !self.chrome.show_settings
    }

    /// 作業面に居る間、Agent パネルは自前のスレッドタブ行を畳む（ペインのタブ行と二重にしない）。
    /// **毎 render で呼ぶ**（`Workspace::render`）。面を切り替える入口は作業タブ・管制トグル・
    /// AI 全画面・復元と複数あり、呼び出し側に配ると必ずどれかを取りこぼす。
    pub(crate) fn sync_work_chrome(&mut self, cx: &mut Context<Self>) {
        let state = (
            self.work_is_visible(),
            self.project_sessions
                .sessions
                .iter()
                .map(|session| session.fleet_agents.len())
                .sum(),
        );
        if self.chrome.work_embedded == Some(state) {
            return;
        }
        self.chrome.work_embedded = Some(state);
        let panels: Vec<_> = self
            .project_sessions
            .sessions
            .iter()
            .flat_map(|session| session.fleet_agents.iter().cloned())
            .collect();
        for panel in panels {
            panel.update(cx, |panel, cx| panel.set_embedded(state.0, cx));
        }
    }

    pub(crate) fn ensure_work_layout(&mut self, cx: &App) {
        let Some(repository) = self.work_repository() else {
            return;
        };
        // 初期復元時は repository ID がまだ未解決のことがある。Space ID で作った仮の机を引き継ぐ。
        if !self
            .chrome
            .work_layout
            .repositories
            .contains_key(&repository)
        {
            if let Some(space) = self.active_slot().map(|slot| slot.task_space.id.0.clone()) {
                if space != repository {
                    if let Some(layout) = self.chrome.work_layout.repositories.remove(&space) {
                        self.chrome
                            .work_layout
                            .repositories
                            .insert(repository.clone(), layout);
                    }
                }
            }
        }
        if self
            .chrome
            .work_layout
            .repositories
            .contains_key(&repository)
        {
            return;
        }
        let Some(slot) = self.active_slot() else {
            return;
        };
        let space = slot.task_space.id.0.clone();
        let column = self
            .chrome
            .work_layout
            .column(space, settings::get(cx).work_tabs_position == "left");
        self.chrome
            .work_layout
            .repositories
            .entry(repository)
            .or_default()
            .open(column, false);
    }

    fn work_location(&self, pane: u64) -> Option<(String, String)> {
        self.chrome
            .work_layout
            .repositories
            .iter()
            .find_map(|(repository, layout)| {
                layout
                    .column_for(pane)
                    .map(|index| (repository.clone(), layout.columns[index].space.clone()))
            })
    }

    /// 別リポジトリへ戻る時はその机の最後の作業場所を優先する。
    pub(crate) fn work_switch_target(&self, index: usize) -> usize {
        if !self.work_is_visible() {
            return index;
        }
        let Some(slot) = self.project_sessions.projects.get(index) else {
            return index;
        };
        if self.work_repository().as_deref() == Some(&slot.task_space.repository_id) {
            return index;
        }
        self.chrome
            .work_layout
            .repositories
            .get(&slot.task_space.repository_id)
            .and_then(|layout| {
                layout
                    .focused
                    .and_then(|id| layout.column_for(id))
                    .map(|i| &layout.columns[i].space)
            })
            .and_then(|space| self.session_index_for_space(&SpaceId(space.clone())))
            .unwrap_or(index)
    }

    pub(crate) fn work_project_changed(&mut self, cx: &App) {
        if !self.work_is_visible() {
            return;
        }
        self.ensure_work_layout(cx);
        let Some(repository) = self.work_repository() else {
            return;
        };
        let Some(slot) = self.active_slot() else {
            return;
        };
        let space = slot.task_space.id.0.clone();
        let column = self
            .chrome
            .work_layout
            .column(space, settings::get(cx).work_tabs_position == "left");
        self.chrome
            .work_layout
            .repositories
            .entry(repository)
            .or_default()
            .open(column, false);
    }

    pub(crate) fn open_work_space(
        &mut self,
        index: usize,
        beside: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(index) else {
            return;
        };
        let repository = if slot.task_space.repository_id.is_empty() {
            slot.task_space.id.0.clone()
        } else {
            slot.task_space.repository_id.clone()
        };
        let space = slot.task_space.id.0.clone();
        let column = self
            .chrome
            .work_layout
            .column(space, settings::get(cx).work_tabs_position == "left");
        let pane = self
            .chrome
            .work_layout
            .repositories
            .entry(repository)
            .or_default()
            .open(column, beside);
        self.chrome.fleet_center_view = FleetCenterView::Work;
        self.chrome.fleet_maximized = None;
        self.focus_work_pane(pane, window, cx);
    }

    fn focus_work_pane(&mut self, pane: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some((repository, space)) = self.work_location(pane) else {
            return;
        };
        let Some(index) = self.session_index_for_space(&SpaceId(space)) else {
            return;
        };
        if let Some(layout) = self.chrome.work_layout.repositories.get_mut(&repository) {
            layout.focused = Some(pane);
        }
        self.switch_project(index, window, cx);
        let surface = self
            .chrome
            .work_layout
            .repositories
            .get(&repository)
            .and_then(|layout| layout.pane(pane))
            .and_then(WorkPane::selected)
            .cloned();
        self.agent_active = surface == Some(WorkSurface::Agent);
        match surface {
            Some(WorkSurface::Agent) => {
                let panel = self.project_sessions.sessions[index].agent_panel.clone();
                window.defer(cx, move |window, cx| {
                    panel.update(cx, |panel, cx| panel.focus_composer(window, cx))
                });
            }
            Some(WorkSurface::Terminal { id }) => {
                if let Some(terminal) = self.project_sessions.sessions[index]
                    .terminal_dock
                    .read(cx)
                    .session(id)
                {
                    window.defer(cx, move |window, cx| {
                        window.focus(&terminal.read(cx).focus_handle(), cx)
                    });
                } else {
                    window.focus(&self.focus_handle, cx);
                }
            }
            Some(WorkSurface::File { path }) => {
                if let Some(tab) = self.tabs.iter().position(|tab| tab.path == path) {
                    self.select_tab(tab, window, cx);
                }
            }
            _ => window.focus(&self.focus_handle, cx),
        }
        self.save_state(cx);
        cx.notify();
    }

    pub(crate) fn reveal_work_file(&mut self, path: PathBuf, cx: &App) {
        if !self.work_is_visible() {
            return;
        }
        self.ensure_work_layout(cx);
        let Some(repository) = self.work_repository() else {
            return;
        };
        let Some(space) = self.active_slot().map(|slot| slot.task_space.id.0.clone()) else {
            return;
        };
        if let Some(layout) = self.chrome.work_layout.repositories.get_mut(&repository) {
            layout.reveal(&space, WorkSurface::File { path });
        }
    }

    fn run_work_action(
        &mut self,
        pane: u64,
        action: WorkAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chrome.work_menu = None;
        let Some((repository, space)) = self.work_location(pane) else {
            return;
        };
        let Some(index) = self.session_index_for_space(&SpaceId(space.clone())) else {
            return;
        };
        self.focus_work_pane(pane, window, cx);
        match action {
            WorkAction::Agent | WorkAction::NewThread | WorkAction::Diff => {
                let surface = if matches!(action, WorkAction::Diff) {
                    self.refresh_git_status_for(index, cx);
                    WorkSurface::Diff
                } else {
                    WorkSurface::Agent
                };
                if matches!(action, WorkAction::NewThread) {
                    self.project_sessions.sessions[index]
                        .agent_panel
                        .clone()
                        .update(cx, |panel, cx| panel.new_thread(cx));
                }
                if let Some(layout) = self.chrome.work_layout.repositories.get_mut(&repository) {
                    layout.reveal(&space, surface);
                }
            }
            WorkAction::Terminal | WorkAction::DockTerminal | WorkAction::Split => {
                let id = self.chrome.work_layout.allocate();
                let dock = self.project_sessions.sessions[index].terminal_dock.clone();
                dock.update(cx, |dock, cx| {
                    if matches!(action, WorkAction::DockTerminal) {
                        dock.detach_active(id, cx);
                    } else {
                        dock.start_session(id, cx);
                    }
                });
                let split_id = self.chrome.work_layout.allocate();
                let layout = self
                    .chrome
                    .work_layout
                    .repositories
                    .get_mut(&repository)
                    .expect("located layout");
                if matches!(action, WorkAction::Split) {
                    let column = layout.column_for(pane).expect("located column");
                    let position = layout.columns[column]
                        .panes
                        .iter()
                        .position(|p| p.id == pane)
                        .unwrap_or(0);
                    let vertical_tabs = layout.pane(pane).is_some_and(|pane| pane.vertical_tabs);
                    layout.columns[column].panes.insert(
                        position + 1,
                        WorkPane {
                            id: split_id,
                            tabs: vec![WorkSurface::Terminal { id }],
                            active: 0,
                            vertical_tabs,
                            weight: 1.0,
                        },
                    );
                    layout.focused = Some(split_id);
                    layout.maximized = None;
                } else {
                    layout.reveal(&space, WorkSurface::Terminal { id });
                }
            }
            WorkAction::File => {
                self.open_file_finder(&FileFinder, window, cx);
            }
            WorkAction::ReturnTerminal | WorkAction::StopTerminal => {
                let surface = self.chrome.work_layout.repositories[&repository]
                    .pane(pane)
                    .and_then(WorkPane::selected)
                    .cloned();
                if let Some(WorkSurface::Terminal { id }) = surface {
                    self.project_sessions.sessions[index]
                        .terminal_dock
                        .update(cx, |dock, cx| {
                            if matches!(action, WorkAction::ReturnTerminal) {
                                dock.attach_session(id, window, cx);
                            } else {
                                dock.terminate_session(id, cx);
                            }
                        });
                    if let Some(layout) = self.chrome.work_layout.repositories.get_mut(&repository)
                    {
                        for column in layout.columns.iter_mut().chain(layout.hidden.values_mut()) {
                            for pane in &mut column.panes {
                                pane.tabs.retain(|tab| tab != &WorkSurface::Terminal { id });
                                pane.active = pane.active.min(pane.tabs.len().saturating_sub(1));
                            }
                        }
                    }
                    if matches!(action, WorkAction::ReturnTerminal) {
                        self.chrome.fleet_bottom_view = FleetBottomView::Terminal;
                        self.chrome.bottom_height = self.chrome.bottom_height.max(180.0);
                    }
                }
            }
            _ => {
                let layout = self
                    .chrome
                    .work_layout
                    .repositories
                    .get_mut(&repository)
                    .expect("located layout");
                match action {
                    WorkAction::Vertical => {
                        if let Some(pane) = layout.pane_mut(pane) {
                            pane.vertical_tabs = !pane.vertical_tabs;
                        }
                    }
                    WorkAction::CloseColumn => layout.close_column(pane),
                    WorkAction::ClosePane => {
                        if let Some(column) = layout.column_for(pane) {
                            if layout.columns[column].panes.len() == 1 {
                                layout.close_column(pane);
                            } else {
                                // タブは隣へ移す。閉じるだけでセッションを見失わない。
                                let at = layout.columns[column]
                                    .panes
                                    .iter()
                                    .position(|p| p.id == pane)
                                    .unwrap_or(0);
                                let removed = layout.columns[column].panes.remove(at);
                                let target =
                                    &mut layout.columns[column].panes[at.saturating_sub(1)];
                                target.tabs.extend(removed.tabs);
                                layout.focused = Some(target.id);
                                layout.maximized = None;
                            }
                        }
                    }
                    WorkAction::CloseTab => {
                        if let Some(pane) = layout.pane_mut(pane) {
                            if pane.active < pane.tabs.len() {
                                pane.tabs.remove(pane.active);
                            }
                            pane.active = pane.active.min(pane.tabs.len().saturating_sub(1));
                        }
                    }
                    WorkAction::Maximize => {
                        layout.maximized = if layout.maximized == Some(pane) {
                            None
                        } else {
                            Some(pane)
                        }
                    }
                    WorkAction::MoveLeft | WorkAction::MoveRight => {
                        if let Some(column) = layout.column_for(pane) {
                            let next = if matches!(action, WorkAction::MoveLeft) {
                                column.saturating_sub(1)
                            } else {
                                (column + 1).min(layout.columns.len() - 1)
                            };
                            layout.columns.swap(column, next);
                        }
                    }
                    WorkAction::MoveTabLeft | WorkAction::MoveTabRight => {
                        if let Some(pane) = layout.pane_mut(pane) {
                            if !pane.tabs.is_empty() {
                                let next = if matches!(action, WorkAction::MoveTabLeft) {
                                    pane.active.saturating_sub(1)
                                } else {
                                    (pane.active + 1).min(pane.tabs.len() - 1)
                                };
                                pane.tabs.swap(pane.active, next);
                                pane.active = next;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let focused = self
            .chrome
            .work_layout
            .repositories
            .get(&repository)
            .and_then(|layout| layout.focused);
        if !matches!(action, WorkAction::File | WorkAction::ReturnTerminal) {
            if let Some(focused) = focused {
                self.focus_work_pane(focused, window, cx);
            }
        }
        self.save_state(cx);
        cx.notify();
    }

    pub(crate) fn work_cycle_tab(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.work_is_visible() {
            return false;
        }
        let Some(repository) = self.work_repository() else {
            return false;
        };
        let Some(layout) = self.chrome.work_layout.repositories.get_mut(&repository) else {
            return false;
        };
        let Some(id) = layout.focused else {
            return false;
        };
        if let Some(pane) = layout.pane_mut(id) {
            if !pane.tabs.is_empty() {
                pane.active = (pane.active as isize + direction)
                    .rem_euclid(pane.tabs.len() as isize) as usize;
            }
        }
        self.focus_work_pane(id, window, cx);
        true
    }

    pub(crate) fn close_work_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.work_is_visible() {
            return false;
        }
        let pane = self
            .work_repository()
            .and_then(|repo| self.chrome.work_layout.repositories.get(&repo))
            .and_then(|layout| layout.focused);
        if let Some(pane) = pane {
            self.run_work_action(pane, WorkAction::CloseTab, window, cx);
            return true;
        }
        false
    }

    pub fn restore_work_layout(&mut self, payload: &str, cx: &mut Context<Self>) {
        if let Ok(saved) = serde_json::from_str::<PersistedState>(payload) {
            self.chrome.work_layout = saved.work_layout;
            self.chrome.work_layout.sanitize();
            self.chrome.fleet_mode = saved.fleet_mode;
            self.chrome.show_herd |= saved.fleet_mode;
            self.chrome.fleet_center_view = match saved.fleet_view.as_str() {
                "control" => FleetCenterView::Control,
                "graph" => FleetCenterView::Graph,
                "work" => FleetCenterView::Work,
                _ => FleetCenterView::Graph,
            };
            self.ensure_work_layout(cx);
            cx.notify();
        }
    }

    fn work_button(
        &self,
        id: impl Into<gpui::ElementId>,
        text: impl Into<SharedString>,
        tip: String,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .px(px(7.))
            .py(px(4.))
            .text_size(px(11.))
            .text_color(self.theme.fg2)
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg2).text_color(self.theme.fg0))
            .child(text.into())
            .tooltip(Tooltip::text(tip, self.theme.clone()))
    }

    /// 作業ツリー（左）: リポジトリ → worktree → 面 の 3 段。**Fleet の主たる移動手段**で、
    /// 各ペインのタブはこれを補うだけの短いもの（UI-SPEC §6.1）。3 段目はその worktree が列として
    /// 出ている時に開く（＝見えている作業の中身が一覧になる。畳んだ worktree は見出しだけ）。
    pub(crate) fn render_work_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let active_repository = self.work_repository();
        let mut repositories: Vec<(String, Vec<usize>)> = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            let key = if slot.task_space.repository_id.is_empty() {
                slot.task_space.id.0.clone()
            } else {
                slot.task_space.repository_id.clone()
            };
            if let Some((_, slots)) = repositories
                .iter_mut()
                .find(|(repository, _)| repository == &key)
            {
                slots.push(index);
            } else {
                repositories.push((key, vec![index]));
            }
        }
        let mut list = div()
            .id("work-repositories")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(6.));
        for (repository, indices) in repositories {
            let index = indices
                .iter()
                .copied()
                .find(|index| {
                    self.project_sessions.projects[*index]
                        .task_space
                        .is_integration()
                })
                .unwrap_or(indices[0]);
            let slot = &self.project_sessions.projects[index];
            let active = active_repository.as_ref() == Some(&repository);
            let expanded = active || self.chrome.work_layout.expanded.contains(&repository);
            let signal_count = indices
                .iter()
                .filter(|index| {
                    self.project_sessions.sessions[**index]
                        .agent_panel
                        .read(cx)
                        .statuses()
                        .iter()
                        .any(|status| status.activity.is_signal())
                })
                .count();
            let key = repository.clone();
            list = list.child(
                div()
                    .id(("work-repo", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(7.))
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg2))
                    .text_color(self.theme.fg0)
                    .child(div().size(px(7.)).rounded_full().bg(slot.color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(slot.name.clone()),
                    )
                    .when(signal_count > 0, |row| {
                        row.child(
                            div()
                                .text_color(self.theme.fg2)
                                .child(format!("◐ {signal_count}")),
                        )
                    })
                    .child(
                        div()
                            .id(("work-repo-fold", index))
                            .text_color(self.theme.fg2)
                            .child(if expanded { "▾" } else { "▸" })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    // アクティブなリポジトリは常に開く（畳んでも中身が見えている）ので折り畳みの対象外。
                                    let expanded = &mut this.chrome.work_layout.expanded;
                                    if let Some(position) =
                                        expanded.iter().position(|item| item == &key)
                                    {
                                        expanded.remove(position);
                                    } else {
                                        expanded.push(key.clone());
                                    }
                                    this.save_state(cx);
                                    cx.notify();
                                }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            let target = this.work_switch_target(index);
                            this.switch_project(target, window, cx);
                            this.ensure_work_layout(cx);
                            if let Some(repo) = this.work_repository() {
                                if let Some(pane) = this
                                    .chrome
                                    .work_layout
                                    .repositories
                                    .get(&repo)
                                    .and_then(|l| l.focused)
                                {
                                    this.focus_work_pane(pane, window, cx);
                                }
                            }
                        }),
                    ),
            );
            if !expanded {
                continue;
            }
            for index in indices {
                let slot = &self.project_sessions.projects[index];
                let space = slot.task_space.id.0.clone();
                let visible = self
                    .chrome
                    .work_layout
                    .repositories
                    .get(&repository)
                    .is_some_and(|layout| {
                        layout.columns.iter().any(|column| column.space == space)
                    });
                let selected = active && self.project_sessions.active == index;
                let label = slot
                    .branch
                    .clone()
                    .or_else(|| slot.worktree_branch.clone())
                    .unwrap_or_else(|| slot.name.to_string());
                let statuses = self.project_sessions.sessions[index]
                    .agent_panel
                    .read(cx)
                    .statuses();
                let status = statuses
                    .iter()
                    .max_by_key(|status| status.activity.urgency())
                    .map(|status| status.activity);
                list =
                    list.child(
                        div()
                            .id(("work-tree", index))
                            .flex()
                            .items_center()
                            .min_w_0()
                            .pl(px(18.))
                            .pr(px(4.))
                            .py(px(3.))
                            .border_l_2()
                            .border_color(if selected { slot.color } else { self.theme.bg0 })
                            .when(selected, |row| row.bg(self.theme.bg2))
                            .cursor_pointer()
                            .hover(|style| style.bg(self.theme.bg1))
                            .text_color(if visible {
                                self.theme.fg0
                            } else {
                                self.theme.fg2
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(12.))
                                    .text_size(px(9.))
                                    .text_color(self.theme.fg2)
                                    .child(if visible { "▾" } else { "▸" }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(format!("⎇ {label}")),
                            )
                            .children(status.filter(|activity| activity.is_signal()).map(
                                |activity| {
                                    agent_panel::activity_dot(
                                        ("work-activity", index),
                                        7.0,
                                        slot.color,
                                        activity,
                                    )
                                },
                            ))
                            .child(
                                self.work_button(
                                    ("work-beside", index),
                                    "◫",
                                    i18n::t!("work.open_beside"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.open_work_space(index, true, window, cx);
                                    }),
                                ),
                            )
                            .tooltip(Tooltip::text(
                                slot.worktree.root().display().to_string(),
                                self.theme.clone(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.open_work_space(index, false, window, cx)
                                }),
                            ),
                    );
                if !visible {
                    continue;
                }
                // 3 段目 = その worktree が持っている面。スレッドは中身（digest）ではなく名前と状態だけ
                // 出す（幅が狭い・中身は管制タブと herd が持つ）。
                let active_thread = self.project_sessions.sessions[index]
                    .agent_panel
                    .read(cx)
                    .active_thread();
                let selected_surface = self
                    .chrome
                    .work_layout
                    .repositories
                    .get(&repository)
                    .and_then(|layout| layout.columns.iter().find(|column| column.space == space))
                    .and_then(|column| column.panes.iter().find_map(|pane| pane.selected()));
                for (thread, status) in statuses.iter().enumerate() {
                    let on_screen =
                        selected_surface == Some(&WorkSurface::Agent) && thread == active_thread;
                    list = list.child(
                        self.render_work_surface_row(
                            ("work-thread", index * 64 + thread),
                            status.name.clone(),
                            on_screen,
                            Some((status.color, status.activity)),
                            status
                                .activity
                                .is_signal()
                                .then(|| activity_label(status.activity)),
                            cx.listener(move |this, _, window, cx| {
                                this.open_work_thread(index, thread, window, cx)
                            }),
                        ),
                    );
                }
                let terminals = self.project_sessions.sessions[index]
                    .terminal_dock
                    .read(cx)
                    .detached_sessions();
                for id in terminals {
                    let on_screen = selected_surface == Some(&WorkSurface::Terminal { id });
                    let repository = repository.clone();
                    let space = space.clone();
                    list = list.child(self.render_work_surface_row(
                        ("work-terminal", index * 64 + id as usize),
                        SharedString::from(format!("{} {id}", i18n::t!("work.terminal"))),
                        on_screen,
                        None,
                        None,
                        cx.listener(move |this, _, window, cx| {
                            if let Some(pane) = this
                                .chrome
                                .work_layout
                                .repositories
                                .get_mut(&repository)
                                .and_then(|layout| {
                                    layout.reveal(&space, WorkSurface::Terminal { id })
                                })
                            {
                                this.focus_work_pane(pane, window, cx);
                            }
                        }),
                    ));
                }
            }
        }
        div()
            .w(px(self.chrome.explorer_width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(self.theme.bg0)
            .border_r_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .px(px(12.))
                    .py(px(9.))
                    .text_color(self.theme.fg2)
                    .text_size(px(11.))
                    .child(i18n::t!("work.repositories")),
            )
            .child(list)
            .child(
                self.work_button(
                    "work-add",
                    i18n::t!("work.add_worktree"),
                    i18n::t!("work.add_worktree_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.open_worktree_picker(window, cx)),
                ),
            )
            .into_any_element()
    }

    /// 作業ツリーの 3 段目（面）の 1 行。`dot` があればスレッド（状態を形と動きで）、無ければ端末など。
    fn render_work_surface_row(
        &self,
        id: (&'static str, usize),
        label: SharedString,
        on_screen: bool,
        dot: Option<(Hsla, agent_panel::ThreadActivity)>,
        note: Option<SharedString>,
        open: impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .pl(px(36.))
            .pr(px(8.))
            .py(px(3.))
            .text_size(px(11.))
            .when(on_screen, |row| row.bg(self.theme.bg2))
            .text_color(if on_screen {
                self.theme.fg0
            } else {
                self.theme.fg2
            })
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg1))
            .children(dot.map(|(color, activity)| {
                agent_panel::activity_dot(("work-surface-dot", id.1), 7.0, color, activity)
            }))
            .when(dot.is_none(), |row| {
                row.child(div().flex_none().text_color(self.theme.fg2).child("›"))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label),
            )
            .children(note.map(|note| {
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(self.theme.fg2)
                    .child(note)
            }))
            .on_mouse_down(MouseButton::Left, open)
    }

    /// 作業ツリーのスレッド行 → その worktree の列（無ければ開く）の Agent 面へ寄せて、そのスレッドを選ぶ。
    fn open_work_thread(
        &mut self,
        index: usize,
        thread: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(index) else {
            return;
        };
        let repository = if slot.task_space.repository_id.is_empty() {
            slot.task_space.id.0.clone()
        } else {
            slot.task_space.repository_id.clone()
        };
        let space = slot.task_space.id.0.clone();
        let visible = self
            .chrome
            .work_layout
            .repositories
            .get(&repository)
            .is_some_and(|layout| layout.columns.iter().any(|column| column.space == space));
        if !visible {
            self.open_work_space(index, true, window, cx);
        }
        let pane = self
            .chrome
            .work_layout
            .repositories
            .get_mut(&repository)
            .and_then(|layout| layout.reveal(&space, WorkSurface::Agent));
        self.project_sessions.sessions[index]
            .agent_panel
            .clone()
            .update(cx, |panel, cx| panel.focus_thread(thread, cx));
        if let Some(pane) = pane {
            self.focus_work_pane(pane, window, cx);
        }
    }

    pub(crate) fn render_workbench(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let repository = self.work_repository().unwrap_or_default();
        let layout = self
            .chrome
            .work_layout
            .repositories
            .get(&repository)
            .cloned()
            .unwrap_or_default();
        let mut body = div()
            .id("work-columns")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .overflow_x_scroll()
            .p(px(6.));
        if let Some(pane) = layout.maximized.and_then(|id| layout.pane(id)) {
            if let Some(column) = layout.column_for(pane.id) {
                body = body.child(self.render_work_column(
                    &layout.columns[column],
                    &layout,
                    Some(pane.id),
                    cx,
                ));
            }
        } else if layout.grid && layout.columns.len() > 2 {
            body = body.flex_col().overflow_y_scroll();
            for row in layout.columns.chunks(2) {
                body = body.child(
                    div().flex_1().min_h(px(260.)).flex().gap(px(6.)).children(
                        row.iter()
                            .map(|column| self.render_work_column(column, &layout, None, cx)),
                    ),
                );
            }
        } else {
            for (index, column) in layout.columns.iter().enumerate() {
                if index > 0 {
                    body = body.child(self.render_work_resize(
                        repository.clone(),
                        index - 1,
                        None,
                        cx,
                    ));
                }
                body = body.child(self.render_work_column(column, &layout, None, cx));
            }
        }
        if layout.columns.is_empty() {
            body = body.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(self.theme.fg2)
                    .child(i18n::t!("work.empty")),
            );
        }
        let grid = layout.grid;
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(29.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(
                        self.work_button(
                            "work-columns-mode",
                            if grid {
                                i18n::t!("work.grid")
                            } else {
                                i18n::t!("work.columns")
                            },
                            i18n::t!("work.toggle_layout"),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if let Some(layout) =
                                    this.chrome.work_layout.repositories.get_mut(&repository)
                                {
                                    layout.grid = !layout.grid;
                                    layout.maximized = None;
                                }
                                this.save_state(cx);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(div().flex_1())
                    .when(layout.maximized.is_some(), |row| {
                        row.child(
                            self.work_button(
                                "work-unfocus",
                                i18n::t!("work.restore"),
                                i18n::t!("work.restore"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(repo) = this.work_repository() {
                                        if let Some(layout) =
                                            this.chrome.work_layout.repositories.get_mut(&repo)
                                        {
                                            layout.maximized = None;
                                        }
                                    }
                                    this.save_state(cx);
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
            )
            .child(body)
            .on_mouse_move(cx.listener(Self::resize_workbench))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.chrome.work_resize.take().is_some() {
                        this.save_state(cx);
                        cx.notify();
                    }
                }),
            )
            .into_any_element()
    }

    fn render_work_column(
        &self,
        column: &WorkColumn,
        layout: &RepositoryLayout,
        only: Option<u64>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(index) = self.session_index_for_space(&SpaceId(column.space.clone())) else {
            let pane = column.panes.first().map(|p| p.id).unwrap_or(0);
            return div()
                .flex_1()
                .min_w(px(300.))
                .flex()
                .flex_col()
                .child(i18n::t!("work.unavailable"))
                .child(
                    self.work_button(
                        SharedString::from(format!("missing-{pane}")),
                        "×",
                        i18n::t!("work.close_column"),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if let Some((repo, _)) = this.work_location(pane) {
                                if let Some(layout) =
                                    this.chrome.work_layout.repositories.get_mut(&repo)
                                {
                                    layout.close_column(pane);
                                }
                            }
                            this.save_state(cx);
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element();
        };
        let slot = &self.project_sessions.projects[index];
        let label = slot.branch.clone().unwrap_or_else(|| slot.name.to_string());
        let first = column.panes.first().map(|p| p.id).unwrap_or(0);
        // 見出しの 2 行目 = その列がいま映している面（列の見出しだけで中身が読める）。
        let showing = column
            .panes
            .iter()
            .filter_map(WorkPane::selected)
            .map(|surface| self.work_surface_label(index, surface, cx))
            .collect::<Vec<_>>()
            .join(" / ");
        let mut content = div()
            .flex_1()
            .min_w(px(300.))
            .min_h_0()
            .flex()
            .flex_col()
            .bg(self.theme.bg1)
            .map(|mut element| {
                element.style().flex_grow = Some(column.weight);
                element.style().flex_basis = Some(px(0.).into());
                element
            })
            .border_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .py(px(5.))
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(div().w(px(3.)).h(px(24.)).bg(slot.color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(12.))
                                    .text_color(self.theme.fg0)
                                    .child(format!("⎇ {label}")),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(10.))
                                    .text_color(self.theme.fg2)
                                    .child(showing),
                            ),
                    )
                    .children(slot.remote_host.clone().map(|host| {
                        div()
                            .text_size(px(10.))
                            .text_color(self.theme.fg2)
                            .child(host)
                    }))
                    .child(
                        self.work_button(
                            ("work-column-close", index),
                            "×",
                            i18n::t!("work.close_column"),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.run_work_action(first, WorkAction::CloseColumn, window, cx);
                            }),
                        ),
                    ),
            );
        let column_index = layout
            .columns
            .iter()
            .position(|c| c.space == column.space)
            .unwrap_or(0);
        for (position, pane) in column.panes.iter().enumerate() {
            if only.is_some_and(|id| id != pane.id) {
                continue;
            }
            if position > 0 && only.is_none() {
                content = content.child(self.render_work_resize(
                    self.work_repository().unwrap_or_default(),
                    position - 1,
                    Some(column_index),
                    cx,
                ));
            }
            content = content.child(self.render_work_pane(
                index,
                pane,
                layout.focused == Some(pane.id),
                cx,
            ));
        }
        content.into_any_element()
    }

    fn work_surface_label(&self, index: usize, surface: &WorkSurface, cx: &App) -> String {
        match surface {
            // Agent 面の名前はスレッド名そのもの（「監督」「レビュー」…）。タブ行と列見出しで
            // 「どのスレッドを映しているか」がそのまま読める。
            WorkSurface::Agent => self.project_sessions.sessions[index]
                .agent_panel
                .read(cx)
                .active_thread_name()
                .map(|name| name.to_string())
                .unwrap_or_else(|| i18n::t!("work.agent")),
            WorkSurface::Terminal { id } => format!("{} {id}", i18n::t!("work.terminal")),
            WorkSurface::Diff => i18n::t!("work.diff"),
            WorkSurface::File { path } => {
                let dirty = self.project_sessions.sessions[index]
                    .tabs
                    .iter()
                    .find(|tab| &tab.path == path)
                    .and_then(|tab| tab.editor())
                    .is_some_and(|editor| editor.read(cx).buffer().is_dirty());
                format!(
                    "{}{}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    if dirty { " •" } else { "" }
                )
            }
        }
    }

    fn render_work_pane(
        &self,
        index: usize,
        pane: &WorkPane,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = pane.id;
        let repository = self.work_repository().unwrap_or_default();
        let mut tabs = div()
            .id(SharedString::from(format!("work-tabs-{id}")))
            .flex()
            .flex_none()
            .min_w_0()
            .bg(self.theme.bg0)
            .when(pane.vertical_tabs, |tabs| {
                tabs.flex_col()
                    .w(px(150.))
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(self.theme.border)
            })
            .when(!pane.vertical_tabs, |tabs| {
                tabs.h(px(30.))
                    .overflow_x_scroll()
                    .border_b_1()
                    .border_color(self.theme.border)
            });
        for (position, surface) in pane.tabs.iter().enumerate() {
            let repo = repository.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("work-tab-{id}-{position}")))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .h(px(29.))
                    .when(pane.vertical_tabs, |tab| tab.w_full())
                    .text_size(px(11.))
                    .text_color(if position == pane.active {
                        self.theme.fg0
                    } else {
                        self.theme.fg2
                    })
                    // 識別色は横タブなら下線、縦タブなら左バー（UI-SPEC §1.3 の許可リスト内で向きだけ変える）。
                    .when(position == pane.active, |tab| {
                        tab.bg(self.theme.bg2)
                            .border_color(self.project_sessions.projects[index].color)
                            .when(pane.vertical_tabs, |tab| tab.border_l_2())
                            .when(!pane.vertical_tabs, |tab| tab.border_b_2())
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg2))
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(self.work_surface_label(index, surface, cx)),
                    )
                    .tooltip(Tooltip::text(
                        match surface {
                            WorkSurface::File { path } => path.display().to_string(),
                            _ => self.work_surface_label(index, surface, cx),
                        },
                        self.theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            if let Some(pane) = this
                                .chrome
                                .work_layout
                                .repositories
                                .get_mut(&repo)
                                .and_then(|layout| layout.pane_mut(id))
                            {
                                pane.active = position;
                            }
                            this.focus_work_pane(id, window, cx);
                        }),
                    ),
            );
        }
        tabs = tabs.child(
            self.work_button(
                SharedString::from(format!("work-menu-{id}")),
                "＋  ⋯",
                i18n::t!("work.pane_menu"),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.chrome.work_menu = if this
                        .chrome
                        .work_menu
                        .as_ref()
                        .is_some_and(|(pane, _)| *pane == id)
                    {
                        None
                    } else {
                        Some((id, event.position))
                    };
                    cx.notify();
                }),
            ),
        );
        let session = &self.project_sessions.sessions[index];
        let body = match pane.selected() {
            Some(WorkSurface::Agent) => session
                .agent_panel
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            Some(WorkSurface::Terminal { id: terminal_id }) => {
                let terminal_id = *terminal_id;
                if let Some(terminal) = session.terminal_dock.read(cx).session(terminal_id) {
                    terminal
                        .cached(StyleRefinement::default().size_full())
                        .into_any_element()
                } else {
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(12.))
                        .text_color(self.theme.fg2)
                        .child(i18n::t!("work.terminal_stopped"))
                        .child(
                            self.work_button(
                                SharedString::from(format!("work-start-{terminal_id}")),
                                i18n::t!("work.start_terminal"),
                                i18n::t!("work.start_terminal"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.project_sessions.sessions[index].terminal_dock.update(
                                        cx,
                                        |dock, cx| {
                                            dock.start_session(terminal_id, cx);
                                        },
                                    );
                                    this.focus_work_pane(id, window, cx);
                                }),
                            ),
                        )
                        .into_any_element()
                }
            }
            Some(WorkSurface::File { path }) => session
                .tabs
                .iter()
                .find(|tab| &tab.path == path)
                .map(EditorTab::content_element)
                .unwrap_or_else(|| {
                    let path = path.clone();
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            self.work_button(
                                SharedString::from(format!("work-open-file-{id}")),
                                i18n::t!("work.open_file"),
                                i18n::t!("work.open_file"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.focus_work_pane(id, window, cx);
                                    this.open_file(path.clone(), window, cx);
                                }),
                            ),
                        )
                        .into_any_element()
                }),
            Some(WorkSurface::Diff) => session.git_panel.clone().into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(self.theme.fg2)
                .child(i18n::t!("work.empty_pane"))
                .into_any_element(),
        };
        div()
            .id(SharedString::from(format!("work-pane-{id}")))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .relative()
            .flex()
            .when(!pane.vertical_tabs, |pane| pane.flex_col())
            .map(|mut element| {
                element.style().flex_grow = Some(pane.weight);
                element.style().flex_basis = Some(px(0.).into());
                element
            })
            .border_t_1()
            .border_color(if focused {
                self.theme.fg2
            } else {
                self.theme.border
            })
            .child(tabs)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(body)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.chrome.work_menu.take().is_some() {
                                cx.notify();
                            }
                        }),
                    ),
            )
            .when(
                self.chrome
                    .work_menu
                    .as_ref()
                    .is_some_and(|(pane, _)| *pane == id),
                |element| element.child(self.render_work_menu(pane, cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    // 子の選択・入力フォーカスは奪わず、コマンドの宛先だけ明示的に合わせる。
                    if let Some((repo, _)) = this.work_location(id) {
                        if let Some(layout) = this.chrome.work_layout.repositories.get_mut(&repo) {
                            layout.focused = Some(id);
                        }
                    }
                    this.switch_project(index, window, cx);
                    this.agent_active = this
                        .chrome
                        .work_layout
                        .repositories
                        .get(&repository)
                        .and_then(|l| l.pane(id))
                        .and_then(WorkPane::selected)
                        == Some(&WorkSurface::Agent);
                    this.save_state(cx);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn render_work_menu(&self, pane: &WorkPane, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = pane.id;
        let mut actions = vec![
            ("work.agent", WorkAction::Agent),
            ("work.new_thread", WorkAction::NewThread),
            ("work.new_terminal", WorkAction::Terminal),
            ("work.move_terminal", WorkAction::DockTerminal),
            ("work.open_file", WorkAction::File),
            ("work.diff", WorkAction::Diff),
            ("work.split", WorkAction::Split),
            (
                if pane.vertical_tabs {
                    "work.tabs_top"
                } else {
                    "work.tabs_left"
                },
                WorkAction::Vertical,
            ),
            ("work.focus", WorkAction::Maximize),
            ("work.move_left", WorkAction::MoveLeft),
            ("work.move_right", WorkAction::MoveRight),
            ("work.tab_previous", WorkAction::MoveTabLeft),
            ("work.tab_next", WorkAction::MoveTabRight),
            ("work.hide_tab", WorkAction::CloseTab),
            ("work.close_pane", WorkAction::ClosePane),
        ];
        if matches!(pane.selected(), Some(WorkSurface::Terminal { .. })) {
            actions.push(("work.return_terminal", WorkAction::ReturnTerminal));
            actions.push(("work.stop_terminal", WorkAction::StopTerminal));
        }
        div()
            .id(SharedString::from(format!("work-popup-{id}")))
            .absolute()
            .top(px(30.))
            .right(px(3.))
            .w(px(215.))
            .max_h(px(440.))
            .overflow_y_scroll()
            .occlude()
            .bg(self.theme.bg1)
            .border_1()
            .border_color(self.theme.border)
            .shadow_lg()
            .rounded(px(6.))
            .py(px(4.))
            .children(
                actions
                    .into_iter()
                    .enumerate()
                    .map(|(position, (key, action))| {
                        self.work_button(
                            SharedString::from(format!("work-action-{id}-{position}")),
                            i18n::t!(key),
                            i18n::t!(key),
                        )
                        .w_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.run_work_action(id, action.clone(), window, cx);
                            }),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_work_resize(
        &self,
        repository: String,
        first: usize,
        column: Option<usize>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .id(SharedString::from(format!(
                "work-resize-{column:?}-{first}"
            )))
            .flex_none()
            .when(column.is_none(), |bar| {
                bar.w(px(6.)).h_full().cursor(CursorStyle::ResizeLeftRight)
            })
            .when(column.is_some(), |bar| {
                bar.h(px(6.)).w_full().cursor(CursorStyle::ResizeUpDown)
            })
            .hover(|style| style.bg(self.theme.bg3))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let Some(layout) = this.chrome.work_layout.repositories.get(&repository) else {
                        return;
                    };
                    let weights = if let Some(column) = column {
                        layout
                            .columns
                            .get(column)
                            .and_then(|c| c.panes.get(first).zip(c.panes.get(first + 1)))
                            .map(|(a, b)| (a.weight, b.weight))
                    } else {
                        layout
                            .columns
                            .get(first)
                            .zip(layout.columns.get(first + 1))
                            .map(|(a, b)| (a.weight, b.weight))
                    };
                    if let Some((first_weight, second_weight)) = weights {
                        this.chrome.work_resize = Some(WorkResize {
                            repository: repository.clone(),
                            first,
                            column,
                            origin: f32::from(if column.is_some() {
                                event.position.y
                            } else {
                                event.position.x
                            }),
                            first_weight,
                            second_weight,
                        });
                    }
                }),
            )
            .into_any_element()
    }

    fn resize_workbench(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            if self.chrome.work_resize.take().is_some() {
                self.save_state(cx);
                cx.notify();
            }
            return;
        }
        let Some(resize) = &self.chrome.work_resize else {
            return;
        };
        let delta = (f32::from(if resize.column.is_some() {
            event.position.y
        } else {
            event.position.x
        }) - resize.origin)
            / 300.;
        let total = resize.first_weight + resize.second_weight;
        let first_weight = (resize.first_weight + delta).clamp(0.2, total - 0.2);
        let Some(layout) = self
            .chrome
            .work_layout
            .repositories
            .get_mut(&resize.repository)
        else {
            return;
        };
        if let Some(column) = resize.column {
            if let Some(column) = layout.columns.get_mut(column) {
                if resize.first + 1 < column.panes.len() {
                    column.panes[resize.first].weight = first_weight;
                    column.panes[resize.first + 1].weight = total - first_weight;
                }
            }
        } else if resize.first + 1 < layout.columns.len() {
            layout.columns[resize.first].weight = first_weight;
            layout.columns[resize.first + 1].weight = total - first_weight;
        }
        cx.notify();
    }
}
