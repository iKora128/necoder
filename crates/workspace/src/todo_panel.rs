impl Workspace {
    fn toggle_todo_board(&mut self, _: &ToggleTodoBoard, _window: &mut Window, cx: &mut Context<Self>) {
        if self.todo_board.take().is_some() {
            cx.notify();
            return;
        }
        self.git_panel = None;
        self.chrome.show_left = true;
        self.todo_board = Some(TodoBoardState {
            items: Vec::new(),
            plan_busy: false,
            running: HashMap::new(),
            add_input: None,
        });
        self.reload_todo_board(cx);
        cx.notify();
    }

    /// 板を `.shirushi/todos.md` から読み直す（背景・最新勝ち）。
    /// 開いた時・チェック書換後・watch 検知・TurnEnded で呼ぶ＝どの書き手の変更も追従する。
    fn reload_todo_board(&mut self, cx: &mut Context<Self>) {
        if self.todo_board.is_none() {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.spawn(async move |workspace, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { project::todos::read_todos_on(host.as_ref(), &root) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Some(board) = workspace.todo_board.as_mut() {
                    board.items = items;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// チェッククリック: 該当行の `[ ]`↔`[x]` をファイル書き換え（他の書き手と同じ経路）。
    fn toggle_todo_item(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::toggle_todo_on(host.as_ref(), &root, line) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
                workspace.reload_todo_board(cx);
            });
        })
        .detach();
    }

    /// ▶: 項目をアクティブスレッドへ送る。末尾に「完了したら板をチェックせよ」を自動付与し、
    /// エージェント自身が todos.md を書き換える → watch が板へ反映（= チェックがひとりでに入る）。
    fn send_todo_to_agent(&mut self, line: usize, text: String, cx: &mut Context<Self>) {
        let prompt = i18n::t!("todos.send_prompt", "text" => text);
        let color = self.agent_panel.read(cx).active_color();
        self.agent_panel.update(cx, |panel, cx| panel.send_prompt_text(prompt, cx));
        self.chrome.show_right = true;
        if let Some(board) = self.todo_board.as_mut() {
            board.running.insert(line, color);
        }
        cx.notify();
    }

    /// ✨今日の計画: ROADMAP/git status/未消化を `claude -p` に渡して下書きを板へ追記（M12-10）。
    fn run_daily_plan(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        if self.todo_board.as_ref().is_some_and(|board| board.plan_busy) {
            return;
        }
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        if let Some(board) = self.todo_board.as_mut() {
            board.plan_busy = true;
        }
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::daily_plan_on(host.as_ref(), &root) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Some(board) = workspace.todo_board.as_mut() {
                    board.plan_busy = false;
                }
                match result {
                    Ok(count) => workspace.push_toast(
                        SharedString::from(i18n::t!("todos.plan_added", "count" => count)),
                        workspace.accent(),
                        cx,
                    ),
                    Err(error) => workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    ),
                }
                workspace.reload_todo_board(cx);
            });
        })
        .detach();
    }

    /// ＋ でタスク追加の入力を開く（IME 正しい EditorView::plain・Enter 確定 / Esc 取消）。
    fn start_add_todo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.todo_board.is_none() {
            return;
        }
        let theme = self.theme.clone();
        let accent = self.accent();
        let editor = cx.new(|cx| EditorView::plain(theme, accent, true, cx));
        cx.subscribe(&editor, |workspace, _editor, event, cx| match event {
            ComposerEvent::Submit => workspace.confirm_add_todo(cx),
        })
        .detach();
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        if let Some(board) = self.todo_board.as_mut() {
            board.add_input = Some(editor);
        }
        cx.notify();
    }

    /// タスク追加を確定（Enter）。空白は無視。今日の見出し下へ `add_todo_on` で追記し板を読み直す。
    fn confirm_add_todo(&mut self, cx: &mut Context<Self>) {
        let text = self
            .todo_board
            .as_ref()
            .and_then(|board| board.add_input.as_ref())
            .map(|editor| editor.read(cx).plain_text().trim().to_string())
            .unwrap_or_default();
        if let Some(board) = self.todo_board.as_mut() {
            board.add_input = None; // 追記中は閉じる（連続追加は再度 ＋）
        }
        if text.is_empty() {
            cx.notify();
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::add_todo_on(host.as_ref(), &root, &text) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
                workspace.reload_todo_board(cx);
            });
        })
        .detach();
    }

    /// タスク追加を取り消す（Esc）。入力内容は破棄する。
    fn cancel_add_todo(&mut self, cx: &mut Context<Self>) {
        if let Some(board) = self.todo_board.as_mut() {
            if board.add_input.take().is_some() {
                cx.notify();
            }
        }
    }

    /// 左カラムの Todo ボード（explorer/git と同じ幅・M12-10）。
    fn render_todo_board(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let Some(board) = self.todo_board.as_ref() else {
            return div().into_any_element();
        };
        let open_count = board.items.iter().filter(|item| !item.done).count();
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
            .child(div().text_size(px(10.5)).text_color(theme.fg2).child(format!("{open_count}")))
            .child(div().flex_1())
            // ✨ 今日の計画（claude -p）。
            .child(
                div()
                    .id("todos-plan")
                    .px(px(7.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .text_color(if board.plan_busy { theme.fg2 } else { accent })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .child(if board.plan_busy {
                        SharedString::from(i18n::t!("todos.plan_busy"))
                    } else {
                        SharedString::from(i18n::t!("todos.plan"))
                    })
                    .tooltip(Tooltip::text(i18n::t!("todos.plan_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.run_daily_plan(cx)),
                    ),
            )
            // ＋ タスクを追加（インライン入力・#todo-add）。
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
                        cx.listener(|this, _, window, cx| this.start_add_todo(window, cx)),
                    ),
            );
        let mut list = div().flex_1().flex().flex_col().overflow_hidden().py(px(4.));
        if board.items.is_empty() {
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
        for item in &board.items {
            // 見出し（日付）はセクションが変わった時だけ描く。
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
            let running_color = board.running.get(&line).copied();
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
                // ☐/☑（クリック = ファイル書き換え）。
                .child(
                    div()
                        .id(("todo-check", line))
                        .flex_none()
                        .text_color(if done { theme.ok } else { theme.fg2 })
                        .cursor_pointer()
                        .child(mark)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| this.toggle_todo_item(line, cx)),
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
            // 実行中はスレッド色 pulse・そうでなければ hover で ▶。
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
                            cx.listener(move |this, _, _window, cx| {
                                this.send_todo_to_agent(line, text.clone(), cx)
                            }),
                        ),
                );
            }
            list = list.child(row);
        }
        div()
            .w(px(self.chrome.explorer_width))
            .h_full()
            .flex_none()
            .relative() // リサイズハンドルの絶対配置基準
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .child(header)
            // ＋ の追加入力（IME 正しい EditorView・Enter 確定 / Esc 取消）。
            .children(board.add_input.clone().map(|editor| {
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
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _window, cx| {
                        if event.keystroke.key.as_str() == "escape" {
                            this.cancel_add_todo(cx);
                        }
                    }))
                    .child(editor)
            }))
            .child(list)
            .child(self.left_dock_resize_handle(cx))
            .into_any_element()
    }

    // ── diff タブ（M11-9）と hunk 操作（M11-10） ──

    // アクティブファイルの HEAD vs バッファ unified diff を一時タブで開く。
}
