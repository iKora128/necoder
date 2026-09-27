use crate::workspace::*;

impl Workspace {
    /// ⌘W / ⌘⇧T / ⌃Tab 系の宛先が AI スレッドタブか（エディタタブとの振り分け・共通判定）。
    /// AI 全画面中はエディタが不可視なので **agent_active に関わらず常に AI 宛て**。全画面中に
    /// 左ドックを触って agent_active が落ちると、見えている ACP タブに操作が効かなくなる実バグの対策。
    /// 編隊モードは AI 全画面を描かない（render は fleet 優先）ので、この特例も適用しない。
    pub(crate) fn agent_surface_active(&self) -> bool {
        // Chat は会話が常に見えている。右のエディタ領域を触った時だけエディタ宛てになる。
        if self.chat_mode() {
            return self.agent_active || self.tabs.is_empty();
        }
        (self.chrome.agent_full_screen && !self.chrome.fleet_mode)
            || (self.chrome.show_right && self.agent_active)
    }

    pub(crate) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_work_tab(window, cx) {
            return;
        }
        // 最後に触った面が Agent なら AI スレッドタブを、そうでなければエディタタブを閉じる。
        // gpui は no-context バインドを最深で解決する（keymap では分離不能）ので、ここで振り分ける。
        // フォーカス依存だと transcript クリック等で判定を外すため、クリックで確定する agent_active を使う。
        if self.agent_surface_active() {
            self.agent_panel
                .update(cx, |panel, cx| panel.close_active_thread(cx));
            return;
        }
        self.close_active_editor(window, cx);
    }

    /// 次のエディタタブへ（⌘} = ⌘⇧]。末尾で先頭へ回る）。
    /// **常にタブ切替**。かつてはレールを押した後だけプロジェクト切替に化けていたが、押した履歴で
    /// キーの意味が変わる隠れモードなので廃止した。プロジェクト移動は ⌃⌘↑↓ か、レールに
    /// フォーカスを渡した上での ↑/↓（`on_rail_key_down`・2026-09-12）。
    pub(crate) fn select_next_tab(
        &mut self,
        _: &SelectNextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.work_cycle_tab(1, window, cx) {
            return;
        }
        if self.tabs.len() > 1 {
            self.select_tab((self.active_tab + 1) % self.tabs.len(), window, cx);
        }
    }

    /// 前のエディタタブへ（⌘{ = ⌘⇧[。先頭で末尾へ回る）。こちらも常にタブ切替（同上）。
    pub(crate) fn select_prev_tab(
        &mut self,
        _: &SelectPrevTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.work_cycle_tab(-1, window, cx) {
            return;
        }
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
        if self.agent_surface_active() {
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

    /// エージェントを決めて新規スレッド（B28）。使わないと決めたエージェント（O16）なら開かずに知らせる
    /// （keymap.json に残したキーから来た時）。
    pub(crate) fn new_agent_thread_with(&mut self, agent: &str, cx: &mut Context<Self>) {
        if !settings::agent_label_enabled(cx, agent) {
            self.push_toast(
                SharedString::from(i18n::t!("agent.disabled_agent", "agent" => agent)),
                self.accent(),
                cx,
            );
            return;
        }
        if !self.chrome.show_right {
            self.chrome.show_right = true;
        }
        self.agent_panel
            .update(cx, |panel, cx| panel.new_thread_with_agent(agent, cx));
        cx.notify();
    }

    /// 次のタブへ（Chrome 風。⌘⌥→ / ⌃Tab）。⌘W と同じく**最後に触った面で振り分け**:
    /// Agent 面ならスレッドタブ、そうでなければエディタのファイルタブを送る（agent_active）。
    pub(crate) fn select_next_thread(
        &mut self,
        _: &SelectNextThread,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.agent_surface_active() {
            self.select_next_tab(&SelectNextTab, window, cx);
            return;
        }
        self.agent_panel
            .update(cx, |panel, cx| panel.select_next_thread(cx));
        cx.notify();
    }

    /// 前のタブへ（Chrome 風。⌘⌥← / ⌃⇧Tab）。振り分けは [`Self::select_next_thread`] と同じ。
    pub(crate) fn select_prev_thread(
        &mut self,
        _: &SelectPrevThread,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.agent_surface_active() {
            self.select_prev_tab(&SelectPrevTab, window, cx);
            return;
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
        self.toggle_agent_full_screen_state(window, cx);
    }

    /// ボタン・キーバインド共通の全画面切替。AgentPanel を別の親へ移す操作なので、
    /// 幅 invalidation と focus 復元を必ず同じ transaction で行う。
    pub(crate) fn toggle_agent_full_screen_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Chat は最初から会話が中央に居る（全画面にする物が無い）。
        if self.chat_mode() {
            return;
        }
        self.chrome.agent_full_screen = !self.chrome.agent_full_screen;
        if self.chrome.agent_full_screen {
            self.chrome.fleet_mode = false;
        }
        self.chrome.show_right = true; // 全画面から抜けた時にも AI が消えていない
        self.agent_active = true; // ⌘W / thread navigation の宛先を AI に固定
        self.agent_panel
            .update(cx, |panel, cx| panel.parent_width_changed(cx));
        // state の notify 後に focus を予約する。次の frame で AgentPanel が右 dock / 中央の
        // どちらへ移っても、同じ composer FocusHandle が新しい dispatch tree に載る。
        let panel = self.agent_panel.clone();
        window.defer(cx, move |window, cx| {
            panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
        });
        cx.notify();
    }

    /// AI 全画面を畳む（既に通常配置なら何もしない）。
    ///
    /// 全画面中は**中央がまるごと Agent に差し替わる**ので、タブを開いても設定ホームを出しても
    /// 画面には出ない＝「開いたのに何も起きない」に見える（2026-09-11 報告）。エディタ領域を
    /// 前に出す対話操作（ファイルを開く・設定を開く・diff タブ）は必ずここを通す。
    /// 復元・監視など**ユーザーの意思でない経路からは呼ばない**（勝手に全画面が解けるため）。
    pub(crate) fn exit_agent_full_screen(&mut self, cx: &mut Context<Self>) {
        if !self.chrome.agent_full_screen {
            return;
        }
        self.chrome.agent_full_screen = false;
        self.chrome.show_right = true; // 抜けた先で AI が消えていない（トグルと同じ出口）
        self.agent_active = false; // これから見せるエディタ側が ⌘W の宛先
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
        if (self.chrome.resizing_agent
            || self.chrome.resizing_explorer
            || self.chrome.resizing_bottom)
            && event.pressed_button != Some(MouseButton::Left)
        {
            // ウィンドウ外で mouse-up を離してイベントを取りこぼしても、戻ってきた最初の
            // move で必ず解除する。sticky resize のまま通常操作へ戻れない状態を作らない。
            self.chrome.resizing_agent = false;
            self.chrome.resizing_explorer = false;
            self.chrome.resizing_bottom = false;
            cx.notify();
            return;
        }
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
            // 左ドックの幅は窓の状態として残す（次に開いた時も同じ幅）。
            let save_width = self.chrome.resizing_explorer;
            self.chrome.resizing_agent = false;
            self.chrome.resizing_explorer = false;
            self.chrome.resizing_bottom = false;
            if save_width {
                self.save_state(cx);
            }
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
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.chrome.resizing_agent = true;
                            this.chrome.resize_start_x = f32::from(event.position.x);
                            this.chrome.resize_start_width = this.chrome.agent_width;
                            this.agent_active = true;
                            this.agent_panel
                                .update(cx, |panel, cx| panel.focus_composer(window, cx));
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
                    Ok(Some(launch)) => {
                        // remote の launch は `ssh -tt …` 自体が接続先で cwd を決めるので、
                        // ローカルのパスを渡すと意味が無い（むしろ壊す）。
                        // 一方 **ローカルで launch が返るのは Windows の既定シェル指定**（§W4）で、
                        // こちらは cwd を渡さないと project root で開かない。
                        let cwd = (!slot.worktree.host().is_remote()).then_some(root);
                        (cwd, Some((launch.program, launch.args)))
                    }
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
        // 開発用（debug のみ）: NECODER_TERM_SCRIPT="<sh の文>" で起動時にその文を実行してから
        // rc を読まない zsh へ（下線・色・リンクなど端末の描画を offscreen で撮るためのフック）。
        #[cfg(debug_assertions)]
        let shell = match std::env::var("NECODER_TERM_SCRIPT") {
            Ok(script) if !script.is_empty() && shell.is_none() => Some((
                "/bin/sh".to_string(),
                vec!["-c".to_string(), format!("{script}; exec zsh -f")],
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

    /// アクティブなエディタタブを閉じて隣へ移る（⌘W）。ピン留めしたタブは閉じずに知らせる（O26）。
    pub(crate) fn close_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if tab.pinned {
            let color = self.accent();
            self.push_toast(i18n::t!("tabs.pinned_kept").into(), color, cx);
            return;
        }
        self.close_tab_at(self.active_tab, window, cx);
    }

    /// `index` 番目のタブを閉じる。前面でプロセスが動いている端末のタブなら先に確かめる（O24・
    /// `terminal_tabs`）。
    pub(crate) fn close_tab_at(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_tabs_asking(vec![index], window, cx);
    }

    /// `index` 番目のタブを確かめずに閉じ、アクティブを隣へ寄せる。閉じたファイルは ⌘⇧T 用に履歴へ
    /// 積み、LSP には didClose を送る。端末のタブは端末も終える。最後の 1 枚を閉じると空状態（分割も畳む）。
    pub(crate) fn close_tab_now(
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
        // 変更レビューは session が持ち続ける（Fleet の「変更」タブと共用）。閉じたら読み込みを止める（R02）。
        if let TabContent::Review(review) = &tab.content {
            self.review_tab_closed(review, cx);
        }
        if !tab.transient {
            self.recently_closed_files.push(tab.path.clone());
            // 画像タブは didOpen していないので didClose も送らない。
            if tab.editor().is_some() {
                self.lsp_did_close(&tab.path);
            }
        }
        // 端末のタブ: ドックの id から外す＝このタブが最後の持ち主になり、タブと一緒に落ちて止まる。
        // 下ドックへ移した端末は先に `detached` から抜けているので止まらない。
        if let Some((_, session)) = tab.terminal() {
            self.terminal_dock
                .update(cx, |dock, cx| dock.terminate_session(session, cx));
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
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if let Some(editor) = tab.editor().cloned() {
                if editor.read(cx).rendered_html() {
                    editor.update(cx, |editor, cx| editor.set_surface_active(true, true, cx));
                } else {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            } else {
                let handle = tab.focus_handle(cx);
                window.focus(&handle, cx);
                // Web タブはキーを WebView へ渡す（HTML プレビューと同じ）。
                if let Some(web) = tab.web().cloned() {
                    web.update(cx, |web, cx| web.set_surface_active(true, true, cx));
                }
            }
        }
        let selected = self.tabs.get(self.active_tab).map(|tab| tab.path.clone());
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            slot.explorer.selected = selected;
        }
        self.sync_active_slot();
        self.push_active_diagnostics(cx);
        self.refresh_git_status(cx);
        self.save_state(cx);
        cx.notify();
    }

    /// `index` 番目を残して他を全部閉じる（タブメニュー）。ピン留めは残す（O26）。後ろから閉じて
    /// 添字のズレを避ける。
    pub(crate) fn close_other_tabs(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        let targets: Vec<usize> = (0..self.tabs.len())
            .filter(|target| *target != index && !self.tabs[*target].pinned)
            .collect();
        self.close_tabs_asking(targets, window, cx);
    }

    /// `index` より右のタブを全部閉じる（タブメニュー）。ピン留めは残す（O26）。
    pub(crate) fn close_tabs_to_right(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        let targets: Vec<usize> = ((index + 1)..self.tabs.len())
            .filter(|target| !self.tabs[*target].pinned)
            .collect();
        self.close_tabs_asking(targets, window, cx);
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
        let Some((handle, path, editor, web, terminal)) = self.tabs.get(index).map(|tab| {
            (
                tab.focus_handle(cx),
                tab.path.clone(),
                tab.editor().cloned(),
                tab.web().cloned(),
                tab.is_terminal(),
            )
        }) else {
            return;
        };
        self.active_tab = index;
        // 端末のタブはファイルではない（作業面・⌘P の最近・エクスプローラの選択に出さない・O24）。
        if !terminal {
            self.reveal_work_file(path.clone(), cx);
        }
        // 移った先のエディタの衝突の印を数える（O19・E05・帯をすぐ出す）。
        if let Some(editor) = editor.as_ref() {
            self.refresh_conflicts(editor, cx);
        }
        if let Some(editor) = editor.filter(|editor| editor.read(cx).rendered_html()) {
            editor.update(cx, |editor, cx| editor.set_surface_active(true, true, cx));
        } else {
            window.focus(&handle, cx);
            // Web タブはキーを WebView へ渡す（HTML プレビューと同じ）。
            if let Some(web) = web {
                web.update(cx, |web, cx| web.set_surface_active(true, true, cx));
            }
        }
        let active = self.project_sessions.active;
        let file_position = self.file_position_of_tab(index);
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            if !terminal {
                slot.explorer.note_opened(&path); // ⌘P の「最近開いた」の先頭へ（D19）
                slot.explorer.selected = Some(path);
            }
            // タブ列の位置ではなく、ファイルの並びの中の位置（端末のタブを数えない・O24）。
            slot.active_file = file_position;
        }
        self.push_active_diagnostics(cx);
        self.save_state(cx);
        cx.notify();
    }

    /// タブを `from` から `to` へ移動する（ドラッグ並べ替え。active は同じタブを指し続ける）。
    /// ピン留めの区切りは越えない（ピン留めはピン留めの中・そうでないタブはその外へ寄せる・O26）。
    pub(crate) fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if from >= count || to >= count || from == to {
            return;
        }
        let to = self.clamp_tab_move(from, to);
        self.reorder_tab(from, to);
        self.sync_active_slot();
        self.save_state(cx);
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
        // 画像タブは分割複製の対象外（Buffer を持たない）。
        if self.active_editor().is_none() {
            return;
        }
        let Some(path) = self.active_tab_path() else {
            return;
        };
        self.open_path_in_split(path, true, window, cx);
    }

    /// タブを右へドラッグして落とした（O24・C03）: そのタブのファイルを右の分割ペインに並べる
    /// （分割が開いていれば中身を差し替える・主ペインのタブはそのまま）。エディタのタブでない物
    /// （画像・PDF・Web・端末・変更レビュー・diff の一時タブ）は並べられないので知らせる。
    pub(crate) fn split_dragged_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .tabs
            .get(index)
            .filter(|tab| tab.editor().is_some() && !tab.transient)
            .map(|tab| tab.path.clone())
        else {
            let color = self.accent();
            self.push_toast(i18n::t!("tabs.split_needs_file").into(), color, cx);
            return;
        };
        self.split_editor = None;
        self.open_path_in_split(path, false, window, cx);
    }

    /// `path` を読んで右分割ペインに開く。local は同期（マイクロ秒）・remote は読みを背景へ
    /// （再接続待ちで固まらない）。`follow_active` = ⌘\ の複製（読む間にタブを移ったら開かない）。
    fn open_path_in_split(
        &mut self,
        path: PathBuf,
        follow_active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        self.split_generation = self.split_generation.wrapping_add(1);
        let generation = self.split_generation;
        if !host.is_remote() {
            match Buffer::from_host(host, &path) {
                Ok(buffer) => self.open_split_editor(buffer, window, cx),
                Err(error) => eprintln!("分割ペインを開けない: {error:#}"),
            }
            return;
        }
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let read_host = host.clone();
        let read_path = path.clone();
        cx.spawn(async move |_workspace, cx| {
            let content = cx
                .background_executor()
                .spawn(async move { read_host.read_file(&read_path) })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                // 読んでいる間に別のプロジェクトへ移った（分割はプロジェクトごと）・次の読みを始めた
                // （続けて落とした・⌘\）・分割を開き閉めした・（⌘\ の複製なら）タブを移った時は、
                // 古い読みで上書きしない。
                let same_project = workspace
                    .active_worktree()
                    .is_some_and(|worktree| worktree.root() == root.as_path());
                if !same_project
                    || workspace.split_generation != generation
                    || workspace.split_editor.is_some()
                    || (follow_active && workspace.active_tab_path() != Some(path.clone()))
                {
                    return;
                }
                match content.and_then(|content| Buffer::from_content(host, &path, content)) {
                    Ok(buffer) => workspace.open_split_editor(buffer, window, cx),
                    Err(error) => eprintln!("分割ペインを開けない: {error:#}"),
                }
            });
        })
        .detach();
    }

    /// 読み終えたバッファで右分割ペインを開いてフォーカスする（`toggle_split` の後半）。
    fn open_split_editor(&mut self, buffer: Buffer, window: &mut Window, cx: &mut Context<Self>) {
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
        // 読みかけの分割も取り消す（閉じた後に開かない）。
        self.split_generation = self.split_generation.wrapping_add(1);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// O24・C03: 接続先（SSH）のファイルは背景で読む。続けて落とせば後の方だけが開き、読んでいる間に
    /// 別のプロジェクトへ移れば、移った先の分割には開かない（遅れて届いた古い読みで上書きしない）。
    #[gpui::test]
    fn late_remote_reads_do_not_land_in_the_wrong_split(cx: &mut gpui::TestAppContext) {
        /// 手元のファイルを読むが、接続先として振る舞う host（読みが背景に回る）。
        struct RemoteLike;
        impl host::Host for RemoteLike {
            fn id(&self) -> &str {
                "remote-like"
            }
            fn display_name(&self) -> &str {
                "devbox"
            }
            fn is_remote(&self) -> bool {
                true
            }
            fn host_for_project(&self, _path: &Path) -> anyhow::Result<Arc<dyn host::Host>> {
                Ok(Arc::new(RemoteLike))
            }
            fn canonicalize(&self, path: &Path) -> anyhow::Result<PathBuf> {
                host::LocalHost.canonicalize(path)
            }
            fn metadata(&self, path: &Path) -> anyhow::Result<host::HostMetadata> {
                host::LocalHost.metadata(path)
            }
            fn read_dir(&self, path: &Path) -> anyhow::Result<Vec<host::HostEntry>> {
                host::LocalHost.read_dir(path)
            }
            fn read_file(&self, path: &Path) -> anyhow::Result<host::FileContent> {
                host::LocalHost.read_file(path)
            }
            fn write_file(
                &self,
                path: &Path,
                bytes: &[u8],
                condition: host::WriteCondition,
            ) -> anyhow::Result<host::FileRevision> {
                host::LocalHost.write_file(path, bytes, condition)
            }
            fn list_files(&self, root: &Path, limit: usize) -> anyhow::Result<Vec<PathBuf>> {
                host::LocalHost.list_files(root, limit)
            }
            fn search_project(
                &self,
                root: &Path,
                spec: &host::TextSearchSpec,
                file_limit: usize,
            ) -> anyhow::Result<Vec<host::TextSearchHit>> {
                host::LocalHost.search_project(root, spec, file_limit)
            }
            fn run_command(&self, spec: &host::CommandSpec) -> anyhow::Result<host::CommandOutput> {
                host::LocalHost.run_command(spec)
            }
            fn spawn_process(&self, spec: &host::CommandSpec) -> anyhow::Result<host::HostProcess> {
                host::LocalHost.spawn_process(spec)
            }
            fn terminal_launch(&self, cwd: &Path) -> anyhow::Result<Option<host::TerminalLaunch>> {
                host::LocalHost.terminal_launch(cwd)
            }
        }

        let base =
            std::env::temp_dir().join(format!("necoder_split_remote_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("remote")).unwrap();
        std::fs::create_dir_all(base.join("local")).unwrap();
        let base = paths::canonicalize(&base).unwrap();
        let remote = base.join("remote");
        std::fs::write(remote.join("a.txt"), "alpha\n").unwrap();
        std::fs::write(remote.join("b.txt"), "beta\n").unwrap();
        std::fs::write(base.join("local").join("c.txt"), "gamma\n").unwrap();
        let settings_path = base.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let sources = vec![
            ProjectSource::new(Arc::new(RemoteLike), remote.clone()),
            ProjectSource::new(host::LocalHost::shared(), base.join("local")),
        ];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new_sources(sources, Theme::dark(), None, cx)
        });
        cx.run_until_parked();
        let split_text = |workspace: &Workspace, cx: &App| {
            workspace
                .split_editor
                .as_ref()
                .map(|split| split.read(cx).plain_text())
        };
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.switch_project(0, window, cx);
            workspace.open_file_sync(remote.join("a.txt"), window, cx);
            workspace.open_file_sync(remote.join("b.txt"), window, cx);
            // 続けて 2 回落とす（どちらも背景で読む）。
            workspace.split_dragged_tab(0, window, cx);
            workspace.split_dragged_tab(1, window, cx);
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(
                split_text(workspace, cx).as_deref(),
                Some("beta\n"),
                "後に落とした方"
            );
            // 落として、読み終わる前に別のプロジェクトへ。
            workspace.split_dragged_tab(0, window, cx);
            workspace.switch_project(1, window, cx);
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            assert_eq!(
                split_text(workspace, cx),
                None,
                "移った先のプロジェクトの分割には開かない"
            );
        });
        let _ = std::fs::remove_dir_all(&base);
    }

    /// O24・C03: タブを右へドラッグして落とすと、そのファイルを右の分割ペインに並べる（主ペインの
    /// タブと選択はそのまま・開いていれば差し替える）。ファイルでないタブは並べずに知らせる。
    #[gpui::test]
    fn a_dragged_tab_opens_on_the_right(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!("necoder_split_drop_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
        std::fs::write(root.join("b.txt"), "beta\n").unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.open_file_sync(root.join("a.txt"), window, cx);
            workspace.open_file_sync(root.join("b.txt"), window, cx);
            assert_eq!(workspace.active_tab, 1);
            let split_text = |workspace: &Workspace, cx: &App| {
                workspace
                    .split_editor
                    .as_ref()
                    .map(|split| split.read(cx).plain_text())
            };

            workspace.split_dragged_tab(0, window, cx);
            assert_eq!(split_text(workspace, cx).as_deref(), Some("alpha\n"));
            assert_eq!(workspace.tabs.len(), 2, "主ペインのタブはそのまま");
            assert_eq!(workspace.active_tab, 1);

            workspace.split_dragged_tab(1, window, cx);
            assert_eq!(
                split_text(workspace, cx).as_deref(),
                Some("beta\n"),
                "差し替える"
            );

            workspace
                .terminal_dock
                .update(cx, |dock, _cx| dock.use_test_terminals());
            workspace.new_terminal_tab(&NewTerminalTab, window, cx);
            workspace.split_dragged_tab(2, window, cx);
            assert_eq!(split_text(workspace, cx).as_deref(), Some("beta\n"));
            assert!(workspace
                .notifications
                .toasts
                .iter()
                .any(|toast| toast.text.as_ref() == i18n::t!("tabs.split_needs_file")));
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
