impl Workspace {
    fn close_hover(&mut self, cx: &mut Context<Self>) {
        if self.hover.take().is_some() {
            // 進行中の応答も無効化する。
            self.hover_generation = self.hover_generation.wrapping_add(1);
            cx.notify();
        }
    }

    /// エディタの確定入力（[`EditorInputEvent::Typed`]）→ 補完の自動トリガ（M10）。
    /// 識別子文字 = 開いていれば絞り込み・閉じていれば新語でポップアップ。`.`/`::` = 新規要求。
    /// その他 = 閉じる。Esc で閉じた語は語頭が変わるまで再表示しない。
    fn on_editor_typed(
        &mut self,
        editor: &Entity<EditorView>,
        event: &EditorInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match event {
            EditorInputEvent::Typed(text) => text,
            EditorInputEvent::HunkClicked { hunk, position } => {
                if self.active_editor().as_ref() == Some(editor) {
                    self.on_hunk_clicked(hunk.clone(), *position, cx);
                }
                return;
            }
            EditorInputEvent::CaretJumped { from } => {
                // 大距離クリック → 移動前の位置をナビ履歴へ（M10-11）。
                if self.active_editor().as_ref() == Some(editor) {
                    if let Some(path) = self.active_tab_path() {
                        let position = (path, *from);
                        if self.nav_back.last() != Some(&position) {
                            self.nav_back.push(position);
                            if self.nav_back.len() > 100 {
                                self.nav_back.remove(0);
                            }
                            self.nav_forward.clear();
                        }
                    }
                }
                return;
            }
        };
        // アクティブタブ以外（比較ビュー等）では何もしない。
        if self.active_editor().as_ref() != Some(editor) {
            return;
        }
        // タイプしたら hover は消す。
        self.close_hover(cx);
        let (before, word_start) = {
            let view = editor.read(cx);
            (view.text_before_caret(4), view.identifier_prefix_at_caret().0)
        };
        match classify_completion_trigger(text, &before) {
            CompletionTrigger::Identifier => {
                if let Some(state) = self.completion.as_mut() {
                    // 開いている → クライアント側絞り込み（LSP 再要求なし）。
                    if let Some(last) = text.chars().last() {
                        state.prefix.push(last);
                        state.selected = 0;
                        if state.filtered().is_empty() {
                            self.close_completion(window, cx);
                        }
                    }
                    cx.notify();
                    return;
                }
                // 閉じている → Esc した語でなければ自動ポップアップ。
                if self.completion_suppressed_word == Some(word_start) {
                    return;
                }
                self.completion_suppressed_word = None;
                self.request_completion(window, cx);
            }
            CompletionTrigger::Fresh => {
                // `.` / `::` はメンバ/パス補完を新規要求（Esc 抑止も解除）。
                self.completion_suppressed_word = None;
                if self.completion.is_some() {
                    self.close_completion(window, cx);
                }
                self.request_completion(window, cx);
            }
            CompletionTrigger::None => {
                if self.completion.is_some() {
                    self.close_completion(window, cx);
                }
            }
        }
    }

    fn switch_project(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = active_index_after_switch(
            self.project_sessions.active,
            index,
            self.project_sessions.projects.len(),
        ) else {
            return;
        };
        self.project_sessions.active = active;
        self.load_active_slot(window, cx);
        self.save_state();
        cx.notify();
    }

    /// active session を表示対象にする。既存 Entity / process は破棄せず、初回だけタブを復元する。
    fn load_active_slot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.loaded {
            self.open_slot_files(window, cx);
            self.loaded = true;
        }
        self.refresh_git_status(cx);
        self.update_agent_destination(cx);
        if self._watch.is_none() {
            self.start_watcher(cx);
        }
        if self.chrome.show_bottom {
            self.terminal_dock.update(cx, |dock, cx| {
                dock.ensure_active(cx);
            });
        }
    }

    /// Agent パネルの宛先チップにアクティブプロジェクト名・ブランチを反映する。
    fn update_agent_destination(&self, cx: &mut Context<Self>) {
        let (name, branch, host, cwd, files) = match self.active_slot() {
            Some(slot) => {
                // Add context の候補（プロジェクト先頭 60 ファイルの相対パス）。
                let files = slot
                    .worktree
                    .all_files(2000) // ＋context の fuzzy 絞り込み対象（M12-7 で 60→2000）
                    .into_iter()
                    .map(|(_, relative)| SharedString::from(relative))
                    .collect();
                (
                    slot.name.clone(),
                    slot.branch.clone().map(SharedString::from),
                    slot.worktree.host().clone(),
                    Some(slot.worktree.root().to_path_buf()),
                    files,
                )
            }
            None => (
                SharedString::from("—"),
                None,
                host::LocalHost::shared(),
                None,
                Vec::new(),
            ),
        };
        self.agent_panel
            .update(cx, |panel, cx| panel.set_destination(name, branch, host, cwd, files, cx));
    }

    // ── タブ/スレッドのショートカット（⌘W / ⌘⇧A） ──
}
