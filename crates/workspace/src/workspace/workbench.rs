//! リポジトリの机。列は worktree、ペインは表示、セッションは既存 ProjectSession に残す。
use crate::workspace::*;

#[derive(Clone, Copy)]
enum WorkAction {
    Terminal,
    DockTerminal,
    Split,
    Vertical,
    CloseTab,
    ReturnTerminal,
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

    pub(crate) fn work_repository(&self) -> Option<String> {
        self.active_repository_key().map(str::to_string)
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
            self.chrome.fleet_mode,
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
        if self.active_repository_key() == Some(slot.repository_key()) {
            return index;
        }
        self.chrome
            .work_layout
            .repositories
            .get(slot.repository_key())
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
        let repository = slot.repository_key().to_string();
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
            WorkAction::ReturnTerminal => {
                let surface = self.chrome.work_layout.repositories[&repository]
                    .pane(pane)
                    .and_then(WorkPane::selected)
                    .cloned();
                if let Some(WorkSurface::Terminal { id }) = surface {
                    self.project_sessions.sessions[index]
                        .terminal_dock
                        .update(cx, |dock, cx| dock.attach_session(id, window, cx));
                    if let Some(layout) = self.chrome.work_layout.repositories.get_mut(&repository)
                    {
                        for column in layout.columns.iter_mut().chain(layout.hidden.values_mut()) {
                            for pane in &mut column.panes {
                                pane.tabs.retain(|tab| tab != &WorkSurface::Terminal { id });
                                pane.active = pane.active.min(pane.tabs.len().saturating_sub(1));
                            }
                        }
                    }
                    self.chrome.fleet_bottom_view = FleetBottomView::Terminal;
                    self.chrome.bottom_height = self.chrome.bottom_height.max(180.0);
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
                    WorkAction::CloseTab => {
                        if let Some(pane) = layout.pane_mut(pane) {
                            if pane.active < pane.tabs.len() {
                                pane.tabs.remove(pane.active);
                            }
                            pane.active = pane.active.min(pane.tabs.len().saturating_sub(1));
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
        if !matches!(action, WorkAction::ReturnTerminal) {
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
                "graph" => FleetCenterView::Graph,
                "work" => FleetCenterView::Work,
                _ => FleetCenterView::Graph,
            };
            self.ensure_work_layout(cx);
            cx.notify();
        }
    }
}
