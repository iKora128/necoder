use crate::workspace::*;

/// プロジェクト切替フラッシュの表示時間（出現 → 保持 → フェードアウトの全体・ms）。
const PROJECT_FLASH_TOTAL_MS: u64 = 1000;
/// 全体のうちフェードアウト開始位置（0..1）。それまでは不透明で保持する。
const PROJECT_FLASH_HOLD_RATIO: f32 = 0.6;

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

    /// キーボード切替（⌃⌘↑↓ / ⌘1..9）専用の入口: 切替に成功したら行き先の名前を中央にフラッシュ。
    /// レールクリックや ⌘O ピッカーは行き先が目に入っているので出さない（この入口を通らない）。
    pub(crate) fn switch_project_flashed(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.project_sessions.active;
        self.switch_project(index, window, cx);
        if self.project_sessions.active != before {
            self.flash_project_name(cx);
        }
    }

    /// アクティブプロジェクトの名前を中央に大きくフラッシュ表示する（約 1 秒で自動消灯）。
    /// 「今どのプロジェクトに居るか」を色レールに加えて名前で確定させる方向感覚の補助。
    pub(crate) fn flash_project_name(&mut self, cx: &mut Context<Self>) {
        let Some(slot) = self.active_slot() else {
            return;
        };
        let name = slot.name.clone();
        let branch = slot
            .branch
            .clone()
            .or_else(|| slot.worktree_branch.clone())
            .map(SharedString::from);
        let color = slot.color;
        self.overlays.project_flash_gen = self.overlays.project_flash_gen.wrapping_add(1);
        let generation = self.overlays.project_flash_gen;
        self.overlays.project_flash = Some(ProjectFlash {
            name,
            branch,
            color,
            generation,
        });
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(PROJECT_FLASH_TOTAL_MS))
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                // 連打中は最新世代だけが表示を持つ（古いタイマーで新しい表示を消さない）。
                if workspace
                    .overlays
                    .project_flash
                    .as_ref()
                    .is_some_and(|flash| flash.generation == generation)
                {
                    workspace.overlays.project_flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// プロジェクト切替フラッシュの描画（中央・最前面）。マウスは受けない（listener を持つ要素を
    /// 置かない＝下の面の操作を遮らない）。フェードは with_animation 1 回きりで idle 予算を守る。
    pub(crate) fn render_project_flash(&self, _cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let flash = self.overlays.project_flash.as_ref()?;
        let theme = self.theme.clone();
        let hold = PROJECT_FLASH_HOLD_RATIO;
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(20.))
                        .px(px(40.))
                        .py(px(24.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(12.))
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(8.),
                            gpui::hsla(0., 0., 0., 0.4),
                        )
                        .blur_radius(px(24.))])
                        // 左の色バー = プロジェクト色（識別への集約・UI-SPEC §1.3。面は塗らない）。
                        .child(div().flex_none().w(px(5.)).h(px(64.)).rounded(px(2.)).bg(flash.color))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(44.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.fg0)
                                        .child(flash.name.clone()),
                                )
                                .children(flash.branch.clone().map(|branch| {
                                    div()
                                        .text_size(px(17.))
                                        .text_color(theme.fg2)
                                        .child(SharedString::from(format!("⎇ {branch}")))
                                })),
                        )
                        .with_animation(
                            ("project-flash", flash.generation as usize),
                            Animation::new(std::time::Duration::from_millis(
                                PROJECT_FLASH_TOTAL_MS,
                            )),
                            // 線形 delta を自前で区分する（easing は掛けない）:
                            // ふわっと出て（最初の 15% = 150ms）、保持し、後半 40% でフェードアウト。
                            move |element, delta| {
                                let opacity = if delta < 0.15 {
                                    delta / 0.15
                                } else if delta < hold {
                                    1.0
                                } else {
                                    1.0 - (delta - hold) / (1.0 - hold)
                                };
                                element.opacity(opacity)
                            },
                        ),
                )
                .into_any_element(),
        )
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
            || self.chrome.task_renaming.is_some()
            || self.chrome.control_focus.is_focused(window)
            || self.focus_handle.is_focused(window)
    }

    /// プロジェクト切替後、新 session の「元居た面」へフォーカスを移す。
    /// Agent → composer / 端末 → アクティブ端末 / それ以外（エディタ・左ドック等）→ アクティブ
    /// エディタ（タブが無ければ workspace 本体）。どの分岐でも必ず描画中の要素に着地させる。
    pub(crate) fn focus_session_surface(
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
                if editor.read(cx).rendered_html() {
                    editor.update(cx, |editor, cx| editor.set_surface_active(true, true, cx));
                } else {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
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
