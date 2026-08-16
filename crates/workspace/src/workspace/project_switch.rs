use crate::workspace::*;

impl Workspace {
    pub(crate) fn close_hover(&mut self, cx: &mut Context<Self>) {
        if self.session_mut().hover.take().is_some() {
            // 進行中の応答も無効化する。
            let generation = &mut self.session_mut().hover_generation;
            *generation = generation.wrapping_add(1);
            cx.notify();
        }
    }

    /// エディタの確定入力（[`EditorInputEvent::Typed`]）→ 補完の自動トリガ（M10）。
    /// 識別子文字 = 開いていれば絞り込み・閉じていれば新語でポップアップ。`.`/`::` = 新規要求。
    /// その他 = 閉じる。Esc で閉じた語は語頭が変わるまで再表示しない。
    pub(crate) fn on_editor_typed(
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
                        let area = self.session_mut();
                        if area.nav_back.last() != Some(&position) {
                            area.nav_back.push(position);
                            if area.nav_back.len() > 100 {
                                area.nav_back.remove(0);
                            }
                            area.nav_forward.clear();
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
            (
                view.text_before_caret(4),
                view.identifier_prefix_at_caret().0,
            )
        };
        match classify_completion_trigger(text, &before) {
            CompletionTrigger::Identifier => {
                if let Some(state) = self.session_mut().completion.as_mut() {
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
                if self.session().completion_suppressed_word == Some(word_start) {
                    return;
                }
                self.session_mut().completion_suppressed_word = None;
                self.request_completion(window, cx);
            }
            CompletionTrigger::Fresh => {
                // `.` / `::` はメンバ/パス補完を新規要求（Esc 抑止も解除）。
                self.session_mut().completion_suppressed_word = None;
                if self.session().completion.is_some() {
                    self.close_completion(window, cx);
                }
                self.request_completion(window, cx);
            }
            CompletionTrigger::None => {
                if self.session().completion.is_some() {
                    self.close_completion(window, cx);
                }
            }
        }
    }

    pub(crate) fn switch_project(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active) = active_index_after_switch(
            self.project_sessions.active,
            index,
            self.project_sessions.projects.len(),
        ) else {
            return;
        };
        // ── フォーカス追従の判定（切替前・旧 session がまだ描画されているうちに読む）──
        // 旧 session の面（composer/端末/エディタ）に居たままだと、切替でその要素が描画ツリーから
        // 外れ、GPUI は key dispatch をウィンドウ最上位へフォールバックさせる＝ Workspace の
        // on_action を全て素通りしてショートカットが死ぬ（⌃⌘↑↓で「戻れなくなる」報告・2026-08-17）。
        // 「どの面に居たか」を覚えて切替後に新 session の同じ面へ付け替える。
        let agent_had_focus = self
            .session()
            .agent_panel
            .read(cx)
            .contains_focus(window, cx);
        let terminal_had_focus = self
            .session()
            .terminal_dock
            .read(cx)
            .contains_focus(window, cx);
        // 編隊モードは全 session のパネルが描画され続ける（フォーカスは迷子にならない）ので触らない。
        let follow_focus = !self.chrome.fleet_mode && !self.chrome_owns_focus(window);
        self.project_sessions.active = active;
        self.load_active_slot(window, cx);
        if follow_focus {
            self.focus_session_surface(agent_had_focus, terminal_had_focus, window, cx);
        }
        self.save_state();
        cx.notify();
    }

    /// 切替後も描画され続ける workspace 常設面（picker 等のオーバレイ・管制・herd 改名・本体）が
    /// フォーカスを持っているか。持っている間はフォーカス追従で奪わない（overlay の操作を壊さない）。
    fn chrome_owns_focus(&self, window: &Window) -> bool {
        self.overlays.picker.is_some()
            || self.overlays.color_picker.is_some()
            || self.overlays.ssh_input.is_some()
            || self.overlays.worktree_delete.is_some()
            || self.overlays.rail_menu.is_some()
            || self.overlays.add_project_dialog_open
            || self.chrome.herd_renaming.is_some()
            || self.chrome.control_focus.is_focused(window)
            || self.focus_handle.is_focused(window)
    }

    /// プロジェクト切替後、新 session の「元居た面」へフォーカスを移す。
    /// Agent → composer / 端末 → アクティブ端末 / それ以外（エディタ・左ドック等）→ アクティブ
    /// エディタ（タブが無ければ workspace 本体）。どの分岐でも必ず描画中の要素に着地させる。
    fn focus_session_surface(
        &mut self,
        agent_had_focus: bool,
        terminal_had_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if agent_had_focus {
            let panel = self.session().agent_panel.clone();
            panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
            return;
        }
        if terminal_had_focus {
            if let Some(terminal) = self.session().terminal_dock.read(cx).active_terminal() {
                let handle = terminal.read(cx).focus_handle();
                window.focus(&handle, cx);
                return;
            }
        }
        match self.active_editor() {
            Some(editor) => {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            None => window.focus(&self.focus_handle, cx),
        }
    }

    /// active session を表示対象にする。既存 Entity / process は破棄せず、初回だけタブを復元する。
    pub(crate) fn load_active_slot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session().loaded {
            self.open_slot_files(window, cx);
            self.session_mut().loaded = true;
        }
        self.refresh_git_status(cx);
        self.update_agent_destination(cx);
        if self.session()._watch.is_none() {
            self.start_watcher(cx);
        }
        if self.chrome.show_bottom {
            self.session().terminal_dock.update(cx, |dock, cx| {
                dock.ensure_active(cx);
            });
        }
    }

    /// Agent パネルの宛先チップにアクティブプロジェクト名・ブランチを反映する。
    pub(crate) fn update_agent_destination(&self, cx: &mut Context<Self>) {
        self.update_agent_destination_for(self.project_sessions.active, cx);
    }

    /// Fleet では非 active な TaskSpace の composer も同時に操作できるため、各 AgentPanel に
    /// それぞれの host/cwd/context を焼き付ける。active session の暗黙 Deref は使わない。
    pub(crate) fn update_agent_destination_for(&self, index: usize, cx: &mut Context<Self>) {
        let (name, branch, host, cwd, files) = match self.project_sessions.projects.get(index) {
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
                    slot.branch
                        .clone()
                        .or_else(|| slot.worktree_branch.clone())
                        .map(SharedString::from),
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
        if let Some(session) = self.project_sessions.sessions.get(index) {
            session.agent_panel.update(cx, |panel, cx| {
                panel.set_destination(name, branch, host, cwd, files, cx)
            });
        }
    }

    // ── タブ/スレッドのショートカット（⌘W / ⌘⇧A） ──
}
