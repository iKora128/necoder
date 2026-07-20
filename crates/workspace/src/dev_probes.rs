// ── 開発用プローブ API（debug build 限定） ──
// SHIRUSHI_* 環境変数からオフスクリーン検証を駆動するための入口だけを置く。
// 全 item が `#[cfg(debug_assertions)]`。本番コードをここに置かない（release に混ざる）。

impl Workspace {
    /// 開発用: Agent タブの改名入力を開く（offscreen 検証・#4）。
    #[cfg(debug_assertions)]
    pub fn debug_tab_rename(&mut self, cx: &mut Context<Self>) {
        if !self.chrome.show_right {
            self.chrome.show_right = true;
        }
        self.agent_panel.update(cx, |panel, cx| panel.debug_start_rename(cx));
        cx.notify();
    }

    /// 開発用: スレッド履歴 Picker を開く（offscreen 検証・#5）。
    #[cfg(debug_assertions)]
    pub fn debug_open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_thread_history(&ThreadHistory, window, cx);
    }

    /// 開発用: SSH 入力バーを開く（M13 の描画検証）。
    #[cfg(debug_assertions)]
    pub fn debug_open_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ssh_input(window, cx);
    }

    /// 開発用: SSH ホストピッカーを開く（SHIRUSHI_SSH_HOST_PROBE の描画検証・M13）。
    #[cfg(debug_assertions)]
    pub fn debug_open_ssh_host_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ssh_host_picker(&RemoteSsh, window, cx);
    }

    /// 開発用: ターミナルを開いて file:line リンクのクリック相当イベントを発火（M13 の結線検証）。
    #[cfg(debug_assertions)]
    pub fn debug_terminal_link(&mut self, path: String, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_terminal(&ToggleTerminal, window, cx);
        self.terminal_dock
            .update(cx, |dock, cx| dock.emit_open_path(path, line, cx));
    }

    /// 開発用: ⌘O スイッチャーを開く（M12-12 のオフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_project_switcher(&ProjectSwitcher, window, cx);
    }

    /// 開発用: ⌘⇧P を開き、query で絞り込む（M13 のオフスクリーン検証）。
    /// `confirm` なら先頭候補を確定 = 実際にアクションが dispatch されるところまで通す。
    #[cfg(debug_assertions)]
    pub fn debug_palette_probe(
        &mut self,
        query: String,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_palette(&CommandPalette, window, cx);
        let Some(picker) = self.overlays.picker.clone() else {
            return;
        };
        picker.update(cx, |picker, cx| {
            if !query.is_empty() {
                picker.set_query(query, cx);
            }
            if confirm {
                picker.confirm_selected(cx);
            }
        });
    }

    /// 開発用: Todo ボードを開く（M12-10 のオフスクリーン検証）。
    /// `SHIRUSHI_TODOS_PLAN=1` なら ✨今日の計画も発火、`SHIRUSHI_TODOS_SEND=<line>` なら
    /// その行を ▶ で AI へ送る（受入「チェックがひとりでに入る」の自動 round trip）。
    #[cfg(debug_assertions)]
    pub fn debug_open_todo_board(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.todo_panel.read(cx).open {
            self.toggle_todo_board(&ToggleTodoBoard, window, cx);
        }
        if std::env::var("SHIRUSHI_TODOS_PLAN").is_ok_and(|value| value == "1") {
            self.run_daily_plan_for(self.project_sessions.active, cx);
        }
        if let Ok(line) = std::env::var("SHIRUSHI_TODOS_SEND") {
            if let Ok(line) = line.parse::<usize>() {
                // 板の読み込み（bg）完了を待ってから該当行を送る。
                let Some(handle) = window.window_handle().downcast::<Workspace>() else {
                    return;
                };
                cx.spawn(async move |_workspace, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    let _ = handle.update(cx, |workspace, _window, cx| {
                        let text = workspace
                            .todo_panel
                            .read(cx)
                            .items
                            .iter()
                            .find(|item| item.line == line)
                            .map(|item| item.text.clone());
                        match text {
                            Some(text) => {
                                eprintln!("TODOS_PROBE: ▶ 送信 line={line} text={text}");
                                workspace.send_todo_to_agent_for(workspace.project_sessions.active, line, text, cx);
                            }
                            None => eprintln!("TODOS_PROBE: line={line} が見つからない"),
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// 開発用: 全選択 → ⌘I → 指示を流し込んで実行（M12-8 のオフスクリーン検証）。
    /// `accept` なら提案到着をポーリングして適用 + 保存まで行う（受入の自動 round trip）。
    #[cfg(debug_assertions)]
    pub fn debug_inline_probe(
        &mut self,
        instruction: String,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |view, cx| {
            let end = view.buffer().len_bytes();
            view.select_byte_range(0..end, cx);
        });
        self.open_inline_edit(&InlineEdit, window, cx);
        let Some(state) = self.inline_edit.as_mut() else {
            eprintln!("INLINE_PROBE: 開けなかった（エディタ無し/対象空）");
            return;
        };
        state.instruction = instruction;
        self.execute_inline_edit(window, cx);
        if !accept {
            return;
        }
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            // 500ms 間隔で最大 120s（claude -p は数秒〜十数秒かかる）。
            for _ in 0..240 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let done = handle
                    .update(cx, |workspace, window, cx| {
                        let Some(state) = workspace.inline_edit.as_ref() else {
                            return true; // 閉じられた
                        };
                        if let Some(error) = &state.error {
                            eprintln!("INLINE_PROBE: 失敗 {error}");
                            return true;
                        }
                        if state.proposal.is_some() {
                            workspace.accept_inline_edit(window, cx);
                            workspace.save_active(&SaveActive, window, cx);
                            eprintln!("INLINE_PROBE: 適用+保存した");
                            return true;
                        }
                        false
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// 開発用: アクティブファイルの diff タブを開く（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_open_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_diff_tab(&OpenDiff, window, cx);
    }

    /// 開発用: ⌘⇧O アウトラインを開く（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_outline_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_outline(&OutlineSymbols, window, cx);
    }

    /// 開発用: ⌥⇧F 相当のフォーマット→保存を実行する（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_format_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_format(true, window, cx);
    }

    /// 開発用: 復元バーの「復元」を押す（オフスクリーン検証。pending が無ければ何もしない）。
    #[cfg(debug_assertions)]
    pub fn debug_restore_hot_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hot_exit_pending.is_some() {
            if std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some() {
                eprintln!("hotexit: 自動復元を実行");
            }
            self.restore_hot_exit(window, cx);
        }
    }

    /// 開発用: (row, col) にキャレットを置いて ⌘K ⌘I 相当の hover を出す（オフスクリーン検証）。
    /// キャレット矩形は直近 paint 由来なので、移動後 1 拍おいてから hover を出す。
    #[cfg(debug_assertions)]
    pub fn debug_hover_probe(
        &mut self,
        row: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |view, cx| view.reveal_position(row, column, cx));
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                workspace.show_hover_at_caret(&ShowHover, window, cx);
            });
        })
        .detach();
    }

    /// 開発用: レール関連フローを実コードで駆動しオフスクリーン検証する（M10-2）。
    /// `open-branch:<name>` = ブランチを worktree としてレールに開く（新窓でなくレール）。
    /// `remove-active` = アクティブスロットをレールから外す（隣へビュー張り替えの確認）。
    #[cfg(debug_assertions)]
    pub fn debug_rail_probe(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(branch) = command.strip_prefix("open-branch:") {
            self.open_branch_worktree(branch.to_string(), cx);
        } else if command == "remove-active" {
            self.remove_project_slot(self.project_sessions.active, window, cx);
        }
    }

    /// 開発用: ⌘F バーをクエリ入りで開く（オフスクリーン検証）。`replace` があれば置換行も開く。
    #[cfg(debug_assertions)]
    pub fn debug_open_buffer_search(
        &mut self,
        query: String,
        replace: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_buffer_search_impl(replace.is_some(), window, cx);
        if let Some(state) = self.buffer_search.as_mut() {
            state.query = query;
            if let Some(replace) = replace {
                state.replace = replace;
            }
        }
        self.refresh_buffer_search(true, cx);
    }
}
