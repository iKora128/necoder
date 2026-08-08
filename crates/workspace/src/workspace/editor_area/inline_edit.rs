use crate::workspace::*;

impl Workspace {
    pub(crate) fn open_inline_edit(
        &mut self,
        _: &InlineEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal_focused = self.terminal_dock.read(cx).is_any_focused(window, cx);
        let target = if terminal_focused {
            InlineEditTarget::Terminal
        } else {
            let Some(editor) = self.active_editor() else {
                return;
            };
            let (range, old_text, buffer_version) = {
                let view = editor.read(cx);
                let buffer = view.buffer();
                if buffer.is_read_only() {
                    return; // diff タブ等は対象外
                }
                let selection = buffer
                    .selections()
                    .first()
                    .copied()
                    .unwrap_or(Selection::cursor(0));
                let mut range = selection.range();
                let snapshot = buffer.snapshot();
                if range.is_empty() {
                    // 選択が無ければ現在行全体（改行込み）を対象にする。
                    let point = snapshot.byte_to_point(range.start);
                    let start = snapshot.point_to_byte(editor_core::Point::new(point.row, 0));
                    let end = if point.row + 1 < snapshot.line_count() {
                        snapshot.point_to_byte(editor_core::Point::new(point.row + 1, 0))
                    } else {
                        snapshot.len_bytes()
                    };
                    range = start..end;
                }
                (range.clone(), buffer.text_range(range), buffer.version())
            };
            if old_text.trim().is_empty() {
                self.push_toast(
                    SharedString::from(i18n::t!("inline.empty_target")),
                    self.accent(),
                    cx,
                );
                return;
            }
            InlineEditTarget::Editor {
                range,
                old_text,
                buffer_version,
            }
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.inline_edit = Some(InlineEditState {
            instruction: String::new(),
            focus,
            target,
            busy: false,
            proposal: None,
            generation: 0,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_inline_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.inline_edit.take() else {
            return;
        };
        // フォーカスを元の場所（エディタ / ターミナル）へ返す。
        match state.target {
            InlineEditTarget::Editor { .. } => {
                if let Some(editor) = self.active_editor() {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            InlineEditTarget::Terminal => {
                self.terminal_dock
                    .update(cx, |dock, cx| dock.focus_active_if_present(window, cx));
            }
        }
        cx.notify();
    }

    /// Enter: 指示を `claude -p` へ（背景実行・世代ガード付き）。
    pub(crate) fn execute_inline_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(state) = self.inline_edit.as_mut() else {
            return;
        };
        if state.busy {
            return;
        }
        let instruction = state.instruction.trim().to_string();
        if instruction.is_empty() {
            return;
        }
        state.busy = true;
        state.error = None;
        state.proposal = None;
        state.generation += 1;
        let generation = state.generation;
        let old_text = match &state.target {
            InlineEditTarget::Editor { old_text, .. } => Some(old_text.clone()),
            InlineEditTarget::Terminal => None,
        };
        cx.notify();
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match old_text {
                        Some(old_text) => project::inline_rewrite_on(
                            host.as_ref(),
                            &root,
                            &instruction,
                            &old_text,
                        ),
                        None => project::inline_command_on(host.as_ref(), &root, &instruction),
                    }
                })
                .await;
            let _ = handle.update(cx, |workspace, _window, cx| {
                let Some(state) = workspace.inline_edit.as_mut() else {
                    return; // Esc で閉じた後に届いた古い結果は捨てる
                };
                if state.generation != generation {
                    return;
                }
                state.busy = false;
                match result {
                    Ok(text) => state.proposal = Some(text),
                    Err(error) => state.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 提案を適用。エディタ = 1 Transaction 置換（⌘Z 一発で戻る・version 不一致は安全側で破棄）。
    /// ターミナル = 生成コマンドを入力行へ挿入（実行はユーザーの Enter に委ねる）。
    pub(crate) fn accept_inline_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.inline_edit.take() else {
            return;
        };
        let Some(proposal) = state.proposal else {
            self.inline_edit = Some(state); // 提案が無いのに呼ばれたら何もしない
            return;
        };
        match &state.target {
            InlineEditTarget::Editor {
                range,
                buffer_version,
                ..
            } => {
                let Some(editor) = self.active_editor() else {
                    return;
                };
                let applied = editor.update(cx, |view, cx| {
                    if view.buffer().version() != *buffer_version {
                        return false;
                    }
                    view.apply_lsp_edits(vec![(range.clone(), proposal)], cx);
                    true
                });
                if !applied {
                    self.push_toast(
                        SharedString::from(i18n::t!("inline.buffer_changed")),
                        self.accent(),
                        cx,
                    );
                }
                if let Some(editor) = self.active_editor() {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            InlineEditTarget::Terminal => {
                self.terminal_dock
                    .update(cx, |dock, cx| dock.insert_text(&proposal, window, cx));
            }
        }
        cx.notify();
    }

    pub(crate) fn on_inline_edit_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let has_proposal = self
            .inline_edit
            .as_ref()
            .is_some_and(|state| state.proposal.is_some());
        match event.keystroke.key.as_str() {
            "escape" => self.close_inline_edit(window, cx),
            "enter" => {
                if has_proposal {
                    self.accept_inline_edit(window, cx);
                } else {
                    self.execute_inline_edit(window, cx);
                }
            }
            "backspace" => {
                if let Some(state) = self.inline_edit.as_mut() {
                    if state.proposal.is_some() {
                        // 提案表示中の編集は「指示を直してやり直す」= 提案を捨てて入力へ戻る。
                        state.proposal = None;
                    }
                    state.instruction.pop();
                    cx.notify();
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }
                if let Some(state) = self.inline_edit.as_mut() {
                    if state.proposal.is_some() {
                        state.proposal = None;
                    }
                    state.instruction.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    /// ⌘I のオーバーレイ（入力バー + 状態行 + diff プレビュー）。rename と同じ中央上配置。
    pub(crate) fn render_inline_edit(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.inline_edit.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let display: SharedString = SharedString::from(state.instruction.clone());
        let placeholder = state.instruction.is_empty();
        let input_row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(30.))
            .px(px(10.))
            .text_size(px(12.5))
            .text_color(theme.fg0)
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(accent)
                    .child(SharedString::from(match &state.target {
                        InlineEditTarget::Editor { .. } => i18n::t!("inline.title"),
                        InlineEditTarget::Terminal => i18n::t!("inline.title_terminal"),
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(placeholder, |element| {
                        element
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!(
                                "inline.instruction_placeholder"
                            )))
                    })
                    .when(!placeholder, |element| element.child(display)),
            )
            .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent));
        let mut card = div()
            .w(px(560.))
            .flex()
            .flex_col()
            .bg(theme.bg2)
            .border_1()
            .border_color(accent)
            .rounded(px(8.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            .track_focus(&state.focus)
            .on_key_down(cx.listener(Self::on_inline_edit_key_down))
            .child(input_row);
        if state.busy {
            card = card.child(
                div()
                    .px(px(10.))
                    .pb(px(8.))
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("inline.busy"))),
            );
        }
        if let Some(error) = &state.error {
            card = card.child(
                div()
                    .px(px(10.))
                    .pb(px(8.))
                    .text_size(px(11.))
                    .text_color(theme.err)
                    .child(SharedString::from(error.clone())),
            );
        }
        if let Some(proposal) = &state.proposal {
            // エディタ = その場 diff（- 旧 / + 新・中央省略）/ ターミナル = 生成コマンド 1 行。
            let preview_lines = match &state.target {
                InlineEditTarget::Editor { old_text, .. } => {
                    inline_edit_diff_lines(old_text, proposal, 14)
                }
                InlineEditTarget::Terminal => vec![format!("$ {proposal}")],
            };
            let mut body = div()
                .flex()
                .flex_col()
                .px(px(10.))
                .py(px(6.))
                .border_t_1()
                .border_color(theme.border)
                .font_family("Menlo")
                .text_size(px(11.))
                .max_h(px(240.))
                .overflow_hidden();
            for line in preview_lines {
                let color = match line.chars().next() {
                    Some('+') => theme.ok,
                    Some('-') => theme.err,
                    Some('$') => theme.fg0,
                    _ => theme.fg2,
                };
                body = body.child(
                    div()
                        .whitespace_nowrap()
                        .text_color(color)
                        .child(SharedString::from(line)),
                );
            }
            let actions = div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .px(px(10.))
                .py(px(7.))
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .id("inline-accept")
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .bg(accent)
                        .text_size(px(11.5))
                        .text_color(gpui::white())
                        .cursor_pointer()
                        .child(SharedString::from(i18n::t!("inline.accept")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.accept_inline_edit(window, cx)),
                        ),
                )
                .child(
                    div()
                        .id("inline-reject")
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .child(SharedString::from(i18n::t!("inline.reject")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_inline_edit(window, cx)),
                        ),
                );
            card = card.child(body).child(actions);
        }
        Some(
            div()
                .absolute()
                .top(px(96.))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .child(card)
                .into_any_element(),
        )
    }

    // ── Todo ボード（M12-10） ──

    // レール ☑ / アクション: 板の表示切替。開く時に読み込み、git パネルとは排他。
}
