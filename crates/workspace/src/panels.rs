/// child panel の typed event を Workspace shell へ接続する唯一の登録点。
struct PanelRegistry;

impl PanelRegistry {
    fn bind_session(
        agent: &Entity<AgentPanel>,
        explorer: &Entity<Explorer>,
        git: &Entity<GitPanel>,
        terminal: &Entity<TerminalDock>,
        todo: &Entity<TodoPanel>,
        cx: &mut Context<Workspace>,
    ) {
        cx.subscribe(agent, Workspace::on_panel_event).detach();
        cx.subscribe(explorer, Workspace::on_explorer_event).detach();
        cx.subscribe(git, Workspace::on_git_panel_event).detach();
        cx.subscribe(terminal, Workspace::on_terminal_dock_event).detach();
        cx.subscribe(todo, Workspace::on_todo_panel_event).detach();
    }

    fn bind_search(panel: &Entity<SearchPanel>, cx: &mut Context<Workspace>) {
        cx.subscribe(panel, Workspace::on_search_panel_event).detach();
    }

    fn bind_settings(view: &Entity<settings::SettingsView>, cx: &mut Context<Workspace>) {
        cx.subscribe(view, Workspace::on_settings_view_event).detach();
    }
}

impl Workspace {
    fn on_explorer_event(
        &mut self,
        explorer: Entity<Explorer>,
        event: &explorer::ExplorerEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(session_index) =
            self.project_sessions.sessions.iter().position(|session| session.explorer == explorer)
        else {
            return;
        };
        match event {
            explorer::ExplorerEvent::OpenPath(path) => {
                self.project_sessions.sessions[session_index].pending_navigation = Some((path.clone(), 0, 0));
            }
            explorer::ExplorerEvent::FilesChanged => {
                if let Some(slot) = self.project_sessions.projects.get_mut(session_index) {
                    slot.refresh();
                }
                self.refresh_git_status_for(session_index, cx);
            }
            explorer::ExplorerEvent::Focus if session_index == self.project_sessions.active => {
                self.chrome.show_left = true;
            }
            explorer::ExplorerEvent::Focus => {}
        }
        cx.notify();
    }

    fn on_git_panel_event(
        &mut self,
        panel: Entity<GitPanel>,
        event: &git_ui::GitPanelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(session_index) =
            self.project_sessions.sessions.iter().position(|session| session.git_panel == panel)
        else {
            return;
        };
        match event {
            git_ui::GitPanelEvent::RepositoryChanged => {
                self.refresh_git_status_for(session_index, cx)
            }
            git_ui::GitPanelEvent::OpenDiff(path) => {
                self.project_sessions.sessions[session_index].pending_open_git_diff = Some(path.clone());
            }
            git_ui::GitPanelEvent::OpenWorktree { path, branch } => {
                let host = self
                    .project_sessions
                    .projects
                    .get(session_index)
                    .map(|slot| slot.worktree.host().clone())
                    .unwrap_or_else(host::LocalHost::shared);
                self.open_folder_in_rail(host, path.clone(), branch.clone(), cx);
            }
            git_ui::GitPanelEvent::Toast { message } => {
                let color = self
                    .project_sessions
                    .projects
                    .get(session_index)
                    .map(|slot| slot.color)
                    .unwrap_or_else(|| project_color(0));
                self.push_toast(message.clone(), color, cx);
            }
            git_ui::GitPanelEvent::StageHunk(hunk) => {
                self.project_sessions.sessions[session_index].pending_stage_hunk = Some(hunk.clone());
            }
        }
        cx.notify();
    }
}
