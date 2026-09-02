use crate::workspace::*;

/// Todo panel から shell へ上げる操作。FS・Agent・Toast は panel から直接触らない。
pub(crate) enum TodoPanelEvent {
    ToggleItem { line: usize },
    SendToAgent { line: usize, text: String },
    DailyPlan,
    AddItem { text: String },
}

/// 1 ProjectSession に属する Todo UI。真実は `.necoder/todos.md`。
pub(crate) struct TodoPanel {
    pub(crate) open: bool,
    pub(crate) items: Vec<project::todos::TodoItem>,
    pub(crate) plan_busy: bool,
    pub(crate) running: HashMap<usize, Hsla>,
    pub(crate) add_input: Option<Entity<EditorView>>,
    pub(crate) theme: Theme,
    pub(crate) accent: Hsla,
}

impl TodoPanel {
    pub(crate) fn new(theme: Theme, accent: Hsla) -> Self {
        Self {
            open: false,
            items: Vec::new(),
            plan_busy: false,
            running: HashMap::new(),
            add_input: None,
            theme,
            accent,
        }
    }

    pub(crate) fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if !open {
            self.add_input = None;
        }
        cx.notify();
    }

    pub(crate) fn set_visuals(&mut self, theme: Theme, accent: Hsla) {
        self.theme = theme;
        self.accent = accent;
    }

    pub(crate) fn set_items(
        &mut self,
        items: Vec<project::todos::TodoItem>,
        cx: &mut Context<Self>,
    ) {
        self.items = items;
        cx.notify();
    }

    pub(crate) fn set_plan_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.plan_busy = busy;
        cx.notify();
    }

    pub(crate) fn mark_running(&mut self, line: usize, color: Hsla, cx: &mut Context<Self>) {
        self.running.insert(line, color);
        cx.notify();
    }

    pub(crate) fn clear_running_color(&mut self, color: Hsla, cx: &mut Context<Self>) {
        self.running
            .retain(|_, running_color| *running_color != color);
        cx.notify();
    }

    pub(crate) fn start_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let editor = cx.new(|cx| EditorView::plain(self.theme.clone(), self.accent, true, cx));
        cx.subscribe(&editor, |panel, _editor, event, cx| match event {
            ComposerEvent::Submit => panel.submit_add(cx),
            // 追加入力は固定高（1 行）なので高さ追従は不要。
            ComposerEvent::ContentHeightChanged => {}
        })
        .detach();
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.add_input = Some(editor);
        cx.notify();
    }

    pub(crate) fn submit_add(&mut self, cx: &mut Context<Self>) {
        let text = self
            .add_input
            .as_ref()
            .map(|editor| editor.read(cx).plain_text().trim().to_string())
            .unwrap_or_default();
        self.add_input = None;
        if !text.is_empty() {
            cx.emit(TodoPanelEvent::AddItem { text });
        }
        cx.notify();
    }

    pub(crate) fn cancel_add(&mut self, cx: &mut Context<Self>) {
        if self.add_input.take().is_some() {
            cx.notify();
        }
    }
}

impl EventEmitter<TodoPanelEvent> for TodoPanel {}

impl Render for TodoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent;
        let open_count = self.items.iter().filter(|item| !item.done).count();
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(34.))
            .px(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.fg1)
                    .child(SharedString::from(i18n::t!("todos.title"))),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(format!("{open_count}")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("todos-plan")
                    .px(px(7.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .text_color(if self.plan_busy { theme.fg2 } else { accent })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .child(if self.plan_busy {
                        SharedString::from(i18n::t!("todos.plan_busy"))
                    } else {
                        SharedString::from(i18n::t!("todos.plan"))
                    })
                    .tooltip(Tooltip::text(i18n::t!("todos.plan_tip"), theme.clone()))
                    .when(!self.plan_busy, |element| {
                        element.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _window, cx| cx.emit(TodoPanelEvent::DailyPlan)),
                        )
                    }),
            )
            .child(
                div()
                    .id("todos-add")
                    .px(px(7.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(13.))
                    .text_color(accent)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .child("＋")
                    .tooltip(Tooltip::text(i18n::t!("todos.add_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|panel, _, window, cx| panel.start_add(window, cx)),
                    ),
            );

        let mut list = div()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .py(px(4.));
        if self.items.is_empty() {
            list = list.child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("todos.empty"))),
            );
        }
        let mut last_section: Option<String> = None;
        for item in &self.items {
            if item.section != last_section {
                if let Some(section) = &item.section {
                    list = list.child(
                        div()
                            .px(px(10.))
                            .pt(px(8.))
                            .pb(px(2.))
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(section.clone())),
                    );
                }
                last_section = item.section.clone();
            }
            let line = item.line;
            let text = item.text.clone();
            let done = item.done;
            let running_color = self.running.get(&line).copied();
            let mark = if done { "☑" } else { "☐" };
            let mut row = div()
                .id(("todo-item", line))
                .group("todo-row")
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(10.))
                .py(px(3.))
                .text_size(px(12.))
                .hover(|style| style.bg(theme.bg2))
                .child(
                    div()
                        .id(("todo-check", line))
                        .flex_none()
                        .text_color(if done { theme.ok } else { theme.fg2 })
                        .cursor_pointer()
                        .child(mark)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_, _, _window, cx| {
                                cx.emit(TodoPanelEvent::ToggleItem { line })
                            }),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(if done { theme.fg2 } else { theme.fg0 })
                        .child(SharedString::from(item.text.clone())),
                );
            if let Some(color) = running_color {
                row = row.child(beacon_dot(("todo-running", line), color, true));
            } else if !done {
                row = row.child(
                    div()
                        .id(("todo-send", line))
                        .flex_none()
                        .invisible()
                        .group_hover("todo-row", |style| style.visible())
                        .text_size(px(11.))
                        .text_color(accent)
                        .cursor_pointer()
                        .child("▶")
                        .tooltip(Tooltip::text(i18n::t!("todos.send_tip"), theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_, _, _window, cx| {
                                cx.emit(TodoPanelEvent::SendToAgent {
                                    line,
                                    text: text.clone(),
                                })
                            }),
                        ),
                );
            }
            list = list.child(row);
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .child(header)
            .children(self.add_input.clone().map(|editor| {
                div()
                    .flex_none()
                    .mx(px(8.))
                    .my(px(4.))
                    .px(px(6.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(accent)
                    .bg(theme.bg1)
                    .on_key_down(
                        cx.listener(|panel, event: &gpui::KeyDownEvent, _window, cx| {
                            if event.keystroke.key.as_str() == "escape" {
                                panel.cancel_add(cx);
                            }
                        }),
                    )
                    .child(editor)
            }))
            .child(list)
    }
}

impl Workspace {
    pub(crate) fn todo_session_index(&self, panel: &Entity<TodoPanel>) -> Option<usize> {
        self.project_sessions
            .sessions
            .iter()
            .position(|session| session.todo_panel == *panel)
    }

    pub(crate) fn on_todo_panel_event(
        &mut self,
        panel: Entity<TodoPanel>,
        event: &TodoPanelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(session_index) = self.todo_session_index(&panel) else {
            return;
        };
        match event {
            TodoPanelEvent::ToggleItem { line } => {
                self.toggle_todo_item_for(session_index, *line, cx)
            }
            TodoPanelEvent::SendToAgent { line, text } => {
                self.send_todo_to_agent_for(session_index, *line, text.clone(), cx)
            }
            TodoPanelEvent::DailyPlan => self.run_daily_plan_for(session_index, cx),
            TodoPanelEvent::AddItem { text } => self.add_todo_for(session_index, text.clone(), cx),
        }
    }

    pub(crate) fn toggle_todo_board(
        &mut self,
        _: &ToggleTodoBoard,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.todo_panel.clone();
        let was_open = panel.read(cx).open;
        panel.update(cx, |panel, cx| panel.set_open(!was_open, cx));
        if !was_open {
            self.git_panel
                .update(cx, |panel, cx| panel.set_open(false, cx));
            self.chrome.show_herd = false;
            self.chrome.show_left = true;
            self.reload_todo_board_for(self.project_sessions.active, cx);
        }
        cx.notify();
    }

    pub(crate) fn reload_todo_board_for(&mut self, session_index: usize, cx: &mut Context<Self>) {
        let Some(panel) = self
            .project_sessions
            .sessions
            .get(session_index)
            .map(|session| session.todo_panel.clone())
        else {
            return;
        };
        if !panel.read(cx).open {
            return;
        }
        let Some(worktree) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.worktree.clone())
        else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.spawn(async move |_workspace, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { project::todos::read_todos_on(host.as_ref(), &root) })
                .await;
            panel.update(cx, |panel, cx| panel.set_items(items, cx));
        })
        .detach();
    }

    pub(crate) fn toggle_todo_item_for(
        &mut self,
        session_index: usize,
        line: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.worktree.clone())
        else {
            return;
        };
        let panel = self.project_sessions.sessions[session_index]
            .todo_panel
            .clone();
        let accent = self.project_sessions.projects[session_index].color;
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::toggle_todo_on(host.as_ref(), &root, line) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    workspace.push_toast(SharedString::from(format!("{error:#}")), accent, cx);
                }
                if let Some(index) = workspace.todo_session_index(&panel) {
                    workspace.reload_todo_board_for(index, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn send_todo_to_agent_for(
        &mut self,
        session_index: usize,
        line: usize,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.project_sessions.sessions.get(session_index) else {
            return;
        };
        let prompt = i18n::t!("todos.send_prompt", "text" => text);
        let color = session.agent_panel.read(cx).active_color();
        session
            .agent_panel
            .update(cx, |panel, cx| panel.send_prompt_text(prompt, cx));
        session
            .todo_panel
            .update(cx, |panel, cx| panel.mark_running(line, color, cx));
        self.chrome.show_right = true;
        cx.notify();
    }

    pub(crate) fn run_daily_plan_for(&mut self, session_index: usize, cx: &mut Context<Self>) {
        let Some(worktree) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.worktree.clone())
        else {
            return;
        };
        let panel = self.project_sessions.sessions[session_index]
            .todo_panel
            .clone();
        if panel.read(cx).plan_busy {
            return;
        }
        panel.update(cx, |panel, cx| panel.set_plan_busy(true, cx));
        let accent = self.project_sessions.projects[session_index].color;
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::daily_plan_on(host.as_ref(), &root) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                panel.update(cx, |panel, cx| panel.set_plan_busy(false, cx));
                match result {
                    Ok(count) => workspace.push_toast(
                        SharedString::from(i18n::t!("todos.plan_added", "count" => count)),
                        accent,
                        cx,
                    ),
                    Err(error) => {
                        workspace.push_toast(SharedString::from(format!("{error:#}")), accent, cx)
                    }
                }
                if let Some(index) = workspace.todo_session_index(&panel) {
                    workspace.reload_todo_board_for(index, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn add_todo_for(
        &mut self,
        session_index: usize,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.worktree.clone())
        else {
            return;
        };
        let panel = self.project_sessions.sessions[session_index]
            .todo_panel
            .clone();
        let accent = self.project_sessions.projects[session_index].color;
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::add_todo_on(host.as_ref(), &root, &text) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    workspace.push_toast(SharedString::from(format!("{error:#}")), accent, cx);
                }
                if let Some(index) = workspace.todo_session_index(&panel) {
                    workspace.reload_todo_board_for(index, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn render_todo_board(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = self.todo_panel.clone();
        let accent = self.accent();
        panel.update(cx, |panel, _| panel.set_visuals(self.theme.clone(), accent));
        div()
            .w(px(self.chrome.explorer_width))
            .h_full()
            .flex_none()
            .relative()
            .child(panel)
            .child(self.left_dock_resize_handle(cx))
            .into_any_element()
    }
}
