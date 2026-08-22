use crate::workspace::*;

impl Workspace {
    pub(crate) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        // 最後に触った面が Agent なら AI スレッドタブを、そうでなければエディタタブを閉じる。
        // gpui は no-context バインドを最深で解決する（keymap では分離不能）ので、ここで振り分ける。
        // フォーカス依存だと transcript クリック等で判定を外すため、クリックで確定する agent_active を使う。
        if self.chrome.show_right && self.agent_active {
            self.agent_panel
                .update(cx, |panel, cx| panel.close_active_thread(cx));
            return;
        }
        self.close_active_editor(window, cx);
    }

    /// 次のエディタタブへ（⌘} = ⌘⇧]。末尾で先頭へ回る）。
    pub(crate) fn select_next_tab(
        &mut self,
        _: &SelectNextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() > 1 {
            self.select_tab((self.active_tab + 1) % self.tabs.len(), window, cx);
        }
    }

    /// 前のエディタタブへ（⌘{ = ⌘⇧[。先頭で末尾へ回る）。
    pub(crate) fn select_prev_tab(
        &mut self,
        _: &SelectPrevTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.tabs.len();
        if count > 1 {
            self.select_tab((self.active_tab + count - 1) % count, window, cx);
        }
    }

    /// 直近に閉じたタブを復元する（Chrome の ⌘⇧T）。⌘W と同じく最後に触った面で振り分ける。
    pub(crate) fn restore_closed_tab(
        &mut self,
        _: &RestoreClosedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.show_right && self.agent_active {
            self.agent_panel
                .update(cx, |panel, cx| panel.restore_closed_thread(cx));
            return;
        }
        if let Some(path) = self.recently_closed_files.pop() {
            self.open_file(path, window, cx);
        }
    }

    pub(crate) fn new_agent_thread(
        &mut self,
        _: &NewThread,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.chrome.show_right {
            self.chrome.show_right = true;
        }
        self.agent_panel
            .update(cx, |panel, cx| panel.new_thread(cx));
        cx.notify();
    }

    /// 次の AI スレッドタブへ（Chrome 風。⌘⌥→ / ⌃Tab）。
    pub(crate) fn select_next_thread(
        &mut self,
        _: &SelectNextThread,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.chrome.show_right {
            self.chrome.show_right = true;
        }
        self.agent_panel
            .update(cx, |panel, cx| panel.select_next_thread(cx));
        cx.notify();
    }

    /// 前の AI スレッドタブへ（Chrome 風。⌘⌥← / ⌃⇧Tab）。
    pub(crate) fn select_prev_thread(
        &mut self,
        _: &SelectPrevThread,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.chrome.show_right {
            self.chrome.show_right = true;
        }
        self.agent_panel
            .update(cx, |panel, cx| panel.select_prev_thread(cx));
        cx.notify();
    }

    /// AI 全画面を切り替える（⌃⌘⏎ / パレット「表示: AI を全画面」・2026-07-27）。
    /// **中央のエディタを Agent に差し替えるだけ**で、左ドック（ファイルブラウザ）と下ドック（ターミナル）は
    /// 各自の ON/OFF に従う（2026-08-08 改訂・全画面が全部を消さない）。レイアウトは `Workspace::render`。
    /// 編隊モードとは排他 — どちらも「窓を丸ごと使う」面なので、AI 全画面にしたら編隊は畳む。
    pub(crate) fn toggle_agent_full_screen(
        &mut self,
        _: &ToggleAgentFullScreen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chrome.agent_full_screen = !self.chrome.agent_full_screen;
        if self.chrome.agent_full_screen {
            self.chrome.fleet_mode = false;
            self.chrome.show_right = true; // 全画面から抜けた時に AI が消えていない
            self.agent_active = true; // ⌘W の宛先を AI スレッドに
            let handle = self.agent_panel.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        self.agent_panel
            .update(cx, |panel, cx| panel.parent_width_changed(cx));
        cx.notify();
    }

    /// アクティブプロジェクトを**新しいウィンドウ**で開く（⌘⇧N。ウィンドウモデル §5）。
    pub(crate) fn new_window(&mut self, _: &NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(slot) = self.active_slot() {
            let root = slot.worktree.root().to_path_buf();
            self.open_folder_as_window(root, cx);
        }
    }

    // ── ドックの可変幅（縁ドラッグ）。Agent=左縁 / エクスプローラ=右縁 ──

    pub(crate) fn on_resize_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dx = f32::from(event.position.x) - self.chrome.resize_start_x;
        if self.chrome.resizing_agent {
            // 上限はウィンドウ幅に追従させる（大画面ではもっと左へ広げられる）。固定 900px の
            // 「突っかかり」を廃し、中央エディタに MIN_CENTER_WIDTH だけ残す位置まで広げられる。
            let viewport_width = f32::from(window.viewport_size().width);
            let agent_max = if viewport_width > 0.0 {
                (viewport_width - RAIL_WIDTH - MIN_CENTER_WIDTH).max(AGENT_DOCK_MIN)
            } else {
                AGENT_DOCK_MAX
            };
            // 左縁を左へ動かすと広がる（dx 負 → 幅増）。
            self.chrome.agent_width =
                (self.chrome.resize_start_width - dx).clamp(AGENT_DOCK_MIN, agent_max);
            self.agent_panel
                .update(cx, |panel, cx| panel.parent_width_changed(cx));
            cx.notify();
        } else if self.chrome.resizing_explorer {
            // 右縁を右へ動かすと広がる（dx 正 → 幅増）。
            self.chrome.explorer_width =
                (self.chrome.resize_start_width + dx).clamp(DOCK_MIN, DOCK_MAX);
            cx.notify();
        } else if self.chrome.resizing_bottom {
            // 上縁を上へ動かすと高くなる（dy 負 → 高さ増）。
            let dy = f32::from(event.position.y) - self.chrome.resize_start_y;
            self.chrome.bottom_height =
                (self.chrome.resize_start_height - dy).clamp(BOTTOM_DOCK_MIN, BOTTOM_DOCK_MAX);
            cx.notify();
        }
    }

    pub(crate) fn on_resize_end(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.resizing_agent
            || self.chrome.resizing_explorer
            || self.chrome.resizing_bottom
        {
            self.chrome.resizing_agent = false;
            self.chrome.resizing_explorer = false;
            self.chrome.resizing_bottom = false;
            cx.notify();
        }
    }

    /// 下段ドックの上縁ハンドル（高さドラッグ）。編隊の下段と solo のターミナルで共有する。
    pub(crate) fn render_bottom_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .id("bottom-resize")
            .h(px(RESIZE_HANDLE_WIDTH))
            .w_full()
            .flex_none()
            .cursor(CursorStyle::ResizeUpDown)
            .hover(|style| style.bg(theme.border))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.chrome.resizing_bottom = true;
                    this.chrome.resize_start_y = f32::from(event.position.y);
                    this.chrome.resize_start_height = this.chrome.bottom_height;
                    cx.notify();
                }),
            )
    }

    /// Agent パネルを可変幅コンテナに入れて描く（左縁にリサイズハンドル）。
    pub(crate) fn render_agent_dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .flex_none()
            .w(px(self.chrome.agent_width))
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            // Agent 側を触った → ⌘W の宛先を Agent スレッドに（クリックで確定）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.agent_active = true),
            )
            .child(
                div()
                    .id("agent-resize")
                    .w(px(RESIZE_HANDLE_WIDTH))
                    .h_full()
                    .flex_none()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|style| style.bg(theme.border))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.chrome.resizing_agent = true;
                            this.chrome.resize_start_x = f32::from(event.position.x);
                            this.chrome.resize_start_width = this.chrome.agent_width;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div().flex_1().min_w_0().h_full().child(
                    self.agent_panel
                        .clone()
                        .cached(StyleRefinement::default().flex().flex_col().size_full()),
                ),
            )
    }

    pub(crate) fn toggle_dock(&mut self, dock: Dock, cx: &mut Context<Self>) {
        match dock {
            Dock::Left => self.chrome.show_left = !self.chrome.show_left,
            Dock::Right => self.chrome.show_right = !self.chrome.show_right,
            Dock::Bottom => self.chrome.show_bottom = !self.chrome.show_bottom,
        }
        cx.notify();
    }

    // ── 下ドックのターミナル（M8） ──

    /// project の Host 設定を TerminalDock が扱える launch spec に解決する。
    pub(crate) fn terminal_launch_for(slot: Option<&ProjectSlot>) -> TerminalLaunch {
        let (cwd, shell) = slot
            .map(|slot| {
                let root = slot.worktree.root().to_path_buf();
                match slot.worktree.host().terminal_launch(&root) {
                    Ok(Some(launch)) => (None, Some((launch.program, launch.args))),
                    Ok(None) => (Some(root), None),
                    Err(error) => {
                        eprintln!("remote terminal を起動できない: {error:#}");
                        (None, None)
                    }
                }
            })
            .unwrap_or((None, None));
        // 開発用: NECODER_TERM_ECHO="text" で起動時に text を表示してから shell へ
        // （file:line リンクの下線描画をオフスクリーン検証するためのフック・M13）。
        let shell = match std::env::var("NECODER_TERM_ECHO") {
            Ok(text) if !text.is_empty() && shell.is_none() => Some((
                "/bin/sh".to_string(),
                vec!["-c".to_string(), format!("echo '{text}'; exec zsh -f")],
            )),
            _ => shell,
        };
        TerminalLaunch { cwd, shell }
    }

    /// アクティブなターミナルにフォーカス（キー入力を受ける）。
    pub(crate) fn focus_active_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_dock
            .update(cx, |dock, cx| dock.focus_active(window, cx));
    }

    /// 下ドック（ターミナル）を開閉する。開くときは生成 + フォーカス（キー入力を受ける）。
    pub(crate) fn toggle_terminal(
        &mut self,
        _: &ToggleTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 編隊では下ドックを別に持たず、常設の下段（ニュース/ターミナル）のタブを切り替える。
        // レールの ⌘ ターミナルアイコンが両モードで同じ意味になるように（2026-07-27）。
        if self.chrome.fleet_mode {
            let view = if self.chrome.fleet_bottom_view == FleetBottomView::Terminal {
                FleetBottomView::News
            } else {
                FleetBottomView::Terminal
            };
            self.set_fleet_bottom_view(view, cx);
            if view == FleetBottomView::Terminal {
                self.focus_active_terminal(window, cx);
            }
            return;
        }
        self.chrome.show_bottom = !self.chrome.show_bottom;
        if self.chrome.show_bottom {
            self.focus_active_terminal(window, cx);
        }
        cx.notify();
    }

    /// アクティブなエディタタブを閉じて隣へ移る（⌘W / タブの ×）。
    pub(crate) fn close_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        self.close_tab_at(self.active_tab, window, cx);
    }

    /// `index` 番目のタブを閉じ、アクティブを隣へ寄せる。閉じたファイルは ⌘⇧T 用に履歴へ積み、
    /// LSP には didClose を送る。最後の 1 枚を閉じると空状態（分割も畳む）。
    pub(crate) fn close_tab_at(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        // アクティブタブを閉じるなら ⌘F バー・hover は畳む（対象エディタが消える）。
        if index == self.active_tab {
            self.dismiss_buffer_search(cx);
            self.close_hover(cx);
        }
        let tab = self.tabs.remove(index);
        if !tab.transient {
            self.recently_closed_files.push(tab.path.clone());
            self.lsp_did_close(&tab.path);
        }
        // hot exit: タブを閉じる＝未保存編集の破棄（現仕様）なのでスナップショットも消す。
        if let Some(storage) = self.persistence.storage.clone() {
            let scope = self.active_hot_exit_scope();
            let path = tab.path.clone();
            cx.background_executor()
                .spawn(async move {
                    let _ = storage.remove_hot_exit(&scope, &path);
                })
                .detach();
        }
        // active を有効域へ寄せる。
        if self.tabs.is_empty() {
            self.active_tab = 0;
            self.split_editor = None; // 何も無ければ分割（比較ビュー）も畳む
        } else if index < self.active_tab {
            self.active_tab -= 1;
        } else if index == self.active_tab {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        }
        // 新しいアクティブタブへフォーカス + 診断反映。
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        let selected = self.tabs.get(self.active_tab).map(|tab| tab.path.clone());
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.projects.get_mut(active) {
            slot.explorer.selected = selected;
        }
        self.sync_active_slot();
        self.push_active_diagnostics(cx);
        self.refresh_git_status(cx);
        self.save_state();
        cx.notify();
    }

    /// `index` 番目のタブをアクティブにする（タブクリック・⌘{ / ⌘}・重複オープン時）。
    pub(crate) fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        // 別のタブへ移るなら ⌘F バー・hover は畳む（エディタ毎の状態）。
        if index != self.active_tab {
            self.dismiss_buffer_search(cx);
            self.close_hover(cx);
        }
        let Some((editor, path)) = self
            .tabs
            .get(index)
            .map(|tab| (tab.editor.clone(), tab.path.clone()))
        else {
            return;
        };
        self.active_tab = index;
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.projects.get_mut(active) {
            slot.explorer.selected = Some(path);
            slot.active_file = index;
        }
        self.push_active_diagnostics(cx);
        self.save_state();
        cx.notify();
    }

    /// タブを `from` から `to` へ移動する（ドラッグ並べ替え。active は同じタブを指し続ける）。
    pub(crate) fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if from >= count || to >= count || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        // active が指すタブを追従させる（remove→insert のインデックスずれを補正）。
        self.active_tab = if self.active_tab == from {
            to
        } else {
            let mut active = self.active_tab;
            if from < active {
                active -= 1;
            }
            if to <= active {
                active += 1;
            }
            active
        };
        self.sync_active_slot();
        self.save_state();
        cx.notify();
    }

    /// 右分割ペインを開閉する（⌘\）。開くときは主ペインの開いているファイルを独立エディタで複製する
    /// （比較・参照用の副ビュー。LSP/保存の統合は主ペイン=editor 側が担う）。
    pub(crate) fn toggle_split(
        &mut self,
        _: &SplitRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.split_editor.is_some() {
            self.close_split(window, cx);
            return;
        }
        let Some(path) = self.active_tab_path() else {
            return;
        };
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let buffer = match Buffer::from_host(worktree.host().clone(), &path) {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("分割ペインを開けない: {error:#}");
                return;
            }
        };
        let theme = self.theme.clone();
        let accent = self
            .active_slot()
            .map(|slot| slot.color)
            .unwrap_or_else(|| project_color(0));
        let split = cx.new(|cx| EditorView::new(buffer, theme, accent, cx));
        let handle = split.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.split_editor = Some(split);
        cx.notify();
    }

    pub(crate) fn close_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_editor.take().is_none() {
            return;
        }
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }
}
