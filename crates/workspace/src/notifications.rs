impl Workspace {
    fn on_panel_event(&mut self, panel: Entity<AgentPanel>, event: &agent_panel::PanelEvent, cx: &mut Context<Self>) {
        let session_index = self
            .sessions
            .iter()
            .position(|session| session.agent_panel == panel)
            .unwrap_or(self.active);
        match event {
            agent_panel::PanelEvent::OpenHistoryRequest => {
                // window が無いので次の render で消化する（pending_transient_tab と同じ迂回・#5）。
                self.sessions[session_index].pending_open_history = true;
                cx.notify();
            }
            agent_panel::PanelEvent::TurnEnded { thread, color, summary } => {
                self.sessions[session_index].waiting_thread = None;
                self.push_toast(SharedString::from(format!("● {thread} — {summary}")), *color, cx);
                // Todo ボード: そのスレッドに送った項目の pulse を解除し、板を読み直す
                // （エージェントが todos.md をチェックしたら watch より先に即反映・M12-10）。
                self.sessions[session_index]
                    .todo_panel
                    .update(cx, |panel, cx| panel.clear_running_color(*color, cx));
                self.reload_todo_board_for(session_index, cx);
            }
            agent_panel::PanelEvent::PermissionWaiting { thread, color } => {
                self.sessions[session_index].waiting_thread = Some((thread.clone(), *color));
                self.push_toast(
                    SharedString::from(format!("● {thread} — {}", i18n::t!("agent.waiting_permission"))),
                    *color,
                    cx,
                );
            }
            agent_panel::PanelEvent::OpenDiffRequest { title, old_text, new_text } => {
                // 提案 diff を transient タブでレビュー（M12-6）。window が要るのでイベントから取得不可 →
                // 承認カードはアクティブ窓でしか押せないため、直近 focus の window handle を使う。
                if let Some(diff_text) = project::unified_diff_texts(old_text, new_text, title) {
                    let mut buffer = Buffer::from_str(&diff_text);
                    buffer.set_read_only(true);
                    self.sessions[session_index].pending_transient_tab =
                        Some((PathBuf::from(i18n::t!("difftab.proposal_title", "title" => title)), buffer));
                    cx.notify();
                }
            }
            agent_panel::PanelEvent::FilesTouched { files, color } => {
                for file in files {
                    self.sessions[session_index].agent_touched.insert(file.clone(), *color);
                    // 開いていれば gutter をスレッド色に（生中継の帰属・M12-3）。
                    if let Some(tab) = self.sessions[session_index]
                        .tabs
                        .iter()
                        .find(|tab| &tab.path == file)
                    {
                        let editor = tab.editor.clone();
                        let color = *color;
                        editor.update(cx, |view, cx| view.set_agent_mark_color(Some(color), cx));
                    }
                }
                cx.notify();
            }
        }
    }

    /// トーストを積む（右下・5 秒で自動で消える・UI-SPEC §8）。
    fn push_toast(&mut self, text: SharedString, color: Hsla, cx: &mut Context<Self>) {
        self.notifications.toast_gen = self.notifications.toast_gen.wrapping_add(1);
        let generation = self.notifications.toast_gen;
        self.notifications.toasts.push((text, color, generation));
        if self.notifications.toasts.len() > 4 {
            self.notifications.toasts.remove(0);
        }
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.notifications.toasts.retain(|(_, _, gen)| *gen != generation);
                cx.notify();
            });
        })
        .detach();
    }

    /// トースト描画（右下スタック・M12-5）。
    fn render_toasts(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.notifications.toasts.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        Some(
            div()
                .absolute()
                .bottom(px(38.))
                .right(px(16.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(self.notifications.toasts.iter().map(|(text, color, generation)| {
                    div()
                        .id(("toast", *generation as usize))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(8.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(color.alpha(0.5))
                        .rounded(px(8.))
                        .shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
                        ])
                        .text_size(px(12.))
                        .text_color(theme.fg0)
                        .child(text.clone())
                }))
                .into_any_element(),
        )
    }

    // ── hot exit（クラッシュ耐性・M10。置き場 = Turso storage crate） ──

    // dirty バッファのスナップショットを（2 秒デバウンスで）DB へ書く。クリーンになった分は消す。
    // 編集の notify 毎に呼ばれるが、世代番号で最後の 1 回だけ実行される。書き込みは背景。
}
