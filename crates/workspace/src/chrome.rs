impl Workspace {
    fn on_settings_view_event(
        &mut self,
        _view: Entity<settings::SettingsView>,
        event: &settings::SettingsViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            settings::SettingsViewEvent::RunCommand(command) => {
                self.chrome.pending_settings_command = Some(command.clone());
                cx.notify();
            }
            settings::SettingsViewEvent::OnboardingCompleted => {
                self.chrome.show_settings = false;
                self.celebrate_confetti(cx);
            }
        }
    }

    fn accent(&self) -> Hsla {
        self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0))
    }

    // ── titlebar（UI-SPEC §3） ──

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        // Peacock 相当: titlebar をアクティブプロジェクト色で淡く塗る（窓ごと識別・M13）。
        let accent = self.accent();
        let tint = gpui::Hsla { s: (accent.s + 0.12).min(1.0), a: 0.26, ..accent };
        div()
            .id("titlebar")
            .bg(tint)
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .gap_2()
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .pl(px(TRAFFIC_LIGHT_INSET)) // ネイティブ信号機を避ける
            .pr_2()
            .border_b_1()
            .border_color(theme.border)
            // 窓ドラッグ: down→move で開始（クリックと区別）。ダブルクリックで zoom（Zed 準拠）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.chrome.should_move_window = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.chrome.should_move_window = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _window, _cx| this.chrome.should_move_window = false))
            .on_mouse_move(cx.listener(|this, _, window, _cx| {
                if this.chrome.should_move_window {
                    this.chrome.should_move_window = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _cx| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(self.render_project_pill(cx))
            .child(div().flex_1().h_full()) // 空き＝ドラッグ領域（titlebar 全体で処理）
            // 実行中スレッドの beacon（窓上部から常に見える＝方向感覚の核・UI-SPEC §3）
            .child(self.render_beacons(cx))
            // リモート SSH で開く（M13・GUI 導線。~/.ssh/config のエイリアス/鍵/ProxyJump がそのまま効く）
            .child(
                self.rail_icon(
                    "titlebar-ssh",
                    "icons/server.svg",
                    i18n::t!("ssh.button_tip"),
                    if self.overlays.ssh_connecting { self.accent() } else { theme.fg2 },
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_ssh_host_picker(&RemoteSsh, window, cx)
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pr(px(4.))
                    .child(self.dock_button(Dock::Left, self.chrome.show_left, cx))
                    .child(self.dock_button(Dock::Bottom, self.chrome.show_bottom, cx))
                    .child(self.dock_button(Dock::Right, self.chrome.show_right, cx)),
            )
    }

    /// titlebar の beacon 列（アクティブプロジェクトのスレッド。実行中は色濃く・停止中は淡く）。
    fn render_beacons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let beacons = self.agent_panel.read(cx).beacons();
        div()
            .flex()
            .items_center()
            .gap_3()
            .mr_2()
            .children(beacons.into_iter().enumerate().map(|(index, (name, color, running))| {
                let status = if running { i18n::t!("git.running") } else { i18n::t!("git.idle") };
                let label = format!("{name} — {status}");
                div()
                    .id(("beacon-item", index))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(px(11.))
                    .text_color(if running { theme.fg1 } else { theme.fg2 })
                    .child(beacon_dot(("beacon", index), color, running))
                    .child(name)
                    .tooltip(Tooltip::text(label, theme.clone()))
            }))
    }

    /// プロジェクトピル: 枠 + 「名前 ▾」+「⎇ branch」。名前クリックで ⌘O。
    /// プロジェクト色はレール/キャレット等の許可箇所に集約（左縁チップは廃止・2026-07-17）。
    fn render_project_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let name = self
            .active_slot()
            .map(|slot| slot.name.clone())
            .unwrap_or_else(|| SharedString::from("—"));
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());

        let mut inner = div().flex().items_center().gap(px(11.)).py(px(4.)).px(px(11.)).child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg0)
                .child(format!("{name} ▾")),
        );
        if let Some(branch) = branch {
            inner = inner.child(
                div()
                    .id("branch-pill")
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_color(theme.fg1)
                    .text_size(px(11.5))
                    .rounded(px(4.))
                    .px(px(7.))
                    .py(px(2.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                    .child(div().text_color(theme.fg2).child("⎇"))
                    .child(SharedString::from(branch.to_string()))
                    .tooltip(Tooltip::text(i18n::t!("git.branch_menu_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation(); // ピル全体（⌘O）を起こさない
                            this.toggle_branch_menu(event.position, cx)
                        }),
                    ),
            );
        }

        div()
            .id("project-pill")
            .flex()
            .items_stretch()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|style| style.border_color(theme.fg2).bg(theme.bg1))
            .overflow_hidden()
            .text_size(px(12.))
            .child(inner)
            .tooltip(Tooltip::text(i18n::t!("git.switcher_tip"), theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation(); // titlebar ドラッグを起こさない
                    this.open_project_switcher(&ProjectSwitcher, window, cx)
                }),
            )
    }

    /// ドックトグルの小アイコン（枠 + 位置ストリップで左/右/下を示す）。
    fn dock_button(&self, dock: Dock, active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let strip = || div().bg(theme.fg1);
        let frame = div()
            .w(px(14.))
            .h(px(11.))
            .flex()
            .border_1()
            .border_color(theme.fg2)
            .rounded(px(2.))
            .overflow_hidden();
        let icon = match dock {
            Dock::Left => frame.child(strip().w(px(4.)).h_full()),
            Dock::Right => frame.justify_end().child(strip().w(px(4.)).h_full()),
            Dock::Bottom => frame.flex_col().justify_end().child(strip().h(px(3.5)).w_full()),
        };
        let (id, label): (&'static str, String) = match dock {
            Dock::Left => ("dock-left", i18n::t!("dock.left")),
            Dock::Bottom => ("dock-bottom", i18n::t!("dock.bottom")),
            Dock::Right => ("dock-right", i18n::t!("dock.right")),
        };
        div()
            .id(id)
            .w(px(26.))
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .when(active, |element| element.bg(theme.bg3))
            .hover(|style| style.bg(theme.bg3))
            .child(icon)
            .tooltip(Tooltip::text(label, theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation(); // titlebar ドラッグを起こさない
                    // 下ドックはターミナル生成 + フォーカスを伴うので専用ハンドラへ。
                    if dock == Dock::Bottom {
                        this.toggle_terminal(&ToggleTerminal, window, cx);
                    } else {
                        this.toggle_dock(dock, cx);
                    }
                }),
            )
    }

    // ── タブ列・パンくず（UI-SPEC §5） ──

    /// 主ペインのタブ列（全タブ。クリック切替・× 閉じる・Chrome 風ドラッグ並べ替え・dirty ドット・
    /// git 色貫通）。M10 複数タブ。`agent_panel::render_thread_tabs` と同じ流儀。
    fn render_main_tabstrip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let active_tab = self.active_tab;
        div()
            .flex()
            .items_stretch()
            .h(px(TABSTRIP_HEIGHT))
            .flex_none()
            .bg(theme.bg0)
            .border_b_1()
            .border_color(theme.border)
            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                let is_active = index == active_tab;
                let name = tab
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| i18n::t!("tabs.untitled"));
                let dirty = tab.editor.read(cx).buffer().is_dirty();
                // タブ名も git 状態で色付け（ツリーと同じ色貫通）。
                let status = self.repository.status.get(&tab.path).copied();
                let name_color = status.map(|status| Self::git_tint(&theme, status)).unwrap_or(theme.fg0);
                let drop_highlight = theme.bg2;
                let drag_name = SharedString::from(name.clone());
                let drag_theme = theme.clone();
                div()
                    .id(("editor-tab", index))
                    .flex()
                    .flex_col()
                    .h_full()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    // Zed 流の即時 hover。アクティブは常時 bg1。id で hover 再描画を保証。
                    .hover(|style| style.bg(theme.bg1))
                    .when(is_active, |element| element.bg(theme.bg1))
                    // アクティブタブ上線 = プロジェクト色（UI-SPEC §5）
                    .child(div().h(px(2.)).w_full().bg(if is_active { accent } else { theme.bg0 }))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px(px(14.))
                            .text_size(px(12.))
                            .text_color(if is_active { theme.fg0 } else { theme.fg1 })
                            .when(dirty, |element| {
                                element.child(div().size(px(7.)).rounded(px(3.5)).bg(theme.warn))
                            })
                            .child(div().text_color(name_color).child(SharedString::from(name)))
                            .child(
                                div()
                                    .id(("close-tab", index))
                                    .flex_none()
                                    .px(px(3.))
                                    .rounded(px(4.))
                                    .text_color(theme.fg2)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg2))
                                    .child("×")
                                    .tooltip(Tooltip::text(i18n::t!("tabs.close_tip"), theme.clone()))
                                    // × クリックはタブ切替へ伝播させない。
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.close_tab_at(index, window, cx);
                                        }),
                                    ),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.agent_active = false; // エディタ側を触った → ⌘W の宛先をタブへ
                            this.select_tab(index, window, cx);
                        }),
                    )
                    // Chrome 風ドラッグ並べ替え: タブを掴んで別タブ上で離すと順序が入れ替わる。
                    .on_drag(
                        DraggedEditorTab { index, name: drag_name, theme: drag_theme },
                        |dragged, _offset, _window, cx| cx.new(|_| dragged.clone()),
                    )
                    .drag_over::<DraggedEditorTab>(move |style, _dragged, _window, _cx| {
                        style.bg(drop_highlight)
                    })
                    .on_drop(cx.listener(move |this, dragged: &DraggedEditorTab, _window, cx| {
                        this.move_tab(dragged.index, index, cx);
                    }))
            }))
            .child(div().flex_1())
    }

    /// 右分割ペインのタブ列（単一比較ビュー。× = 分割を閉じる）。
    fn render_split_tabstrip(
        &self,
        editor: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let name = editor
            .read(cx)
            .buffer()
            .path()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| i18n::t!("tabs.untitled"));
        let dirty = editor.read(cx).buffer().is_dirty();
        div()
            .flex()
            .items_stretch()
            .h(px(TABSTRIP_HEIGHT))
            .flex_none()
            .bg(theme.bg0)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("split-tab")
                    .flex()
                    .flex_col()
                    .h_full()
                    .border_r_1()
                    .border_color(theme.border)
                    .bg(theme.bg1)
                    .child(div().h(px(2.)).w_full().bg(accent))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px(px(14.))
                            .text_size(px(12.))
                            .text_color(theme.fg0)
                            .when(dirty, |element| {
                                element.child(div().size(px(7.)).rounded(px(3.5)).bg(theme.warn))
                            })
                            .child(SharedString::from(name))
                            .child(
                                div()
                                    .id("close-split")
                                    .flex_none()
                                    .px(px(3.))
                                    .rounded(px(4.))
                                    .text_color(theme.fg2)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg2))
                                    .child("×")
                                    .tooltip(Tooltip::text(i18n::t!("tabs.close_split_tip"), theme.clone()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| this.close_split(window, cx)),
                                    ),
                            ),
                    ),
            )
            .child(div().flex_1())
    }

    fn render_breadcrumb(&self, editor: &Entity<EditorView>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let path = editor.read(cx).buffer().path().map(Path::to_path_buf);
        let root = self.active_slot().map(|slot| slot.worktree.root().to_path_buf());
        let crumbs = breadcrumb_text(root.as_deref(), path.as_deref());
        div()
            .flex()
            .items_center()
            .h(px(BREADCRUMB_HEIGHT))
            .px(px(14.))
            .flex_none()
            .bg(theme.bg1)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg2)
            .child(SharedString::from(crumbs))
    }

    /// ⌘F インライン検索/置換バー（エディタ右上に浮かせる・M10）。
    /// 行1 = [▸/▾] [クエリ] [n/m] [Aa] [.*] [‹] [›] [×]、行2（置換表示時）= [置換入力] [置換] [全置換]。
    fn render_buffer_search_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.buffer_search.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let show_replace = state.show_replace;
        let editing_replace = state.editing_replace && show_replace;

        // n/m カウンタ（正規表現エラーはここに赤で出す）。
        let (counter, counter_color) = if let Some(error) = &state.error {
            (error.clone(), theme.err)
        } else if state.matches.is_empty() {
            let text = if state.query.is_empty() {
                SharedString::from("")
            } else {
                SharedString::from(i18n::t!("search.no_results"))
            };
            (text, theme.fg2)
        } else {
            let text = format!(
                "{}/{}{}",
                state.current + 1,
                state.matches.len(),
                if state.truncated { "+" } else { "" }
            );
            (SharedString::from(text), theme.fg2)
        };

        // 入力フィールド（クエリ / 置換）。アクティブ側はアクセント枠 + 末尾キャレットバー。
        let field = |id: &'static str, text: &str, placeholder: String, active: bool| {
            let display: SharedString = if text.is_empty() {
                SharedString::from(placeholder)
            } else {
                SharedString::from(text.to_string())
            };
            let text_color = if text.is_empty() { theme.fg2 } else { theme.fg0 };
            div()
                .id(id)
                .flex_1()
                .flex()
                .items_center()
                .h(px(24.))
                .px(px(8.))
                .rounded(px(6.))
                .bg(theme.bg1)
                .border_1()
                .border_color(if active { accent } else { theme.border })
                .overflow_hidden()
                .cursor(CursorStyle::IBeam)
                .text_size(px(12.))
                .text_color(text_color)
                .child(div().overflow_hidden().whitespace_nowrap().child(display))
                .when(active, |element| {
                    element.child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent))
                })
        };

        // 小さな正方形ボタン（トグルチップ / マッチ移動 / 閉じる）。
        let chip = |id: &'static str, label: &'static str, active: bool, tip: SharedString| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(5.))
                .text_size(px(11.))
                .text_color(if active { theme.fg0 } else { theme.fg2 })
                .cursor_pointer()
                .when(active, |element| element.bg(accent.alpha(0.16)))
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(label)
                .tooltip(Tooltip::text(tip, theme.clone()))
        };

        // 置換の実行ボタン（置換 / 全置換）。
        let action_button = |id: &'static str, label: String, tip: SharedString| {
            div()
                .id(id)
                .flex_none()
                .flex()
                .items_center()
                .h(px(22.))
                .px(px(8.))
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(label))
                .tooltip(Tooltip::text(tip, theme.clone()))
        };

        let query_row = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                // ▸/▾ = 置換行の開閉。
                div()
                    .id("bsearch-toggle-replace")
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.))
                    .h(px(24.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(if show_replace { "▾" } else { "▸" })
                    .tooltip(Tooltip::text(i18n::t!("search.toggle_replace"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            if let Some(state) = this.buffer_search.as_mut() {
                                state.show_replace = !state.show_replace;
                                if !state.show_replace {
                                    state.editing_replace = false;
                                }
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(
                field(
                    "bsearch-query",
                    &state.query,
                    i18n::t!("search.find_placeholder"),
                    !editing_replace,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        if let Some(state) = this.buffer_search.as_mut() {
                            state.editing_replace = false;
                        }
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(150.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .text_color(counter_color)
                    .child(counter),
            )
            .child(
                chip(
                    "bsearch-case",
                    "Aa",
                    state.case_sensitive,
                    SharedString::from(i18n::t!("search.case_sensitive")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.toggle_buffer_search_case(cx)),
                ),
            )
            .child(
                chip(
                    "bsearch-regex",
                    ".*",
                    state.is_regex,
                    SharedString::from(i18n::t!("search.use_regex")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.toggle_buffer_search_regex(cx)),
                ),
            )
            .child(
                chip("bsearch-prev", "‹", false, SharedString::from(i18n::t!("search.previous")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.step_buffer_match(-1, cx)),
                    ),
            )
            .child(
                chip("bsearch-next", "›", false, SharedString::from(i18n::t!("search.next")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.step_buffer_match(1, cx)),
                    ),
            )
            .child(
                chip("bsearch-close", "×", false, SharedString::from(i18n::t!("search.close")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_buffer_search(false, window, cx)),
                    ),
            );

        let replace_row = show_replace.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().flex_none().w(px(16.)))
                .child(
                    field(
                        "bsearch-replace",
                        &state.replace,
                        i18n::t!("search.replace_placeholder"),
                        editing_replace,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            if let Some(state) = this.buffer_search.as_mut() {
                                state.editing_replace = true;
                            }
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    action_button(
                        "bsearch-replace-one",
                        i18n::t!("search.replace_one"),
                        SharedString::from(i18n::t!("search.replace_one_tip")),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.replace_current_buffer_match(cx)),
                    ),
                )
                .child(
                    action_button(
                        "bsearch-replace-all",
                        i18n::t!("search.replace_all"),
                        SharedString::from(i18n::t!("search.replace_all_tip")),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.replace_all_buffer_matches(cx)),
                    ),
                )
        });

        let focus = state.focus.clone();
        Some(
            div()
                .id("buffer-search-bar")
                .absolute()
                .top(px(8.))
                .right(px(14.))
                .w(px(460.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(6.))
                .bg(theme.bg2)
                .rounded(px(8.))
                .border_1()
                .border_color(theme.border)
                .shadow(vec![
                    gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.35))
                        .blur_radius(px(18.)),
                ])
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_buffer_search_key_down))
                // バー内クリックはエディタへ通さない + フォーカスを付け直す（⌘W の宛先もエディタ側へ）。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.agent_active = false;
                        if let Some(state) = this.buffer_search.as_ref() {
                            let focus = state.focus.clone();
                            window.focus(&focus, cx);
                        }
                    }),
                )
                .child(query_row)
                .children(replace_row)
                .into_any_element(),
        )
    }

    /// 主ペイン（複数タブ列 + アクティブタブのパンくず + 本体）。⌘F バーは本体右上に浮かせる。
    fn render_main_pane(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(editor) = self.active_editor() else {
            return div().flex_1().into_any_element();
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(self.render_main_tabstrip(cx))
            .child(self.render_breadcrumb(&editor, cx))
            .children(self.render_external_change_bar(cx))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    .child(editor.clone())
                    .children(self.render_buffer_search_bar(cx)),
            )
            .into_any_element()
    }

    /// 右分割ペイン（単一タブ + パンくず + 本体）。
    fn render_split_pane(&self, editor: &Entity<EditorView>, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(self.render_split_tabstrip(editor, cx))
            .child(self.render_breadcrumb(editor, cx))
            .child(div().flex_1().overflow_hidden().child(editor.clone()))
            .into_any_element()
    }

    /// 祝いの紙吹雪を降らせる（~2.2s で自動的に止める）。
    fn celebrate_confetti(&mut self, cx: &mut Context<Self>) {
        self.chrome.confetti = true;
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(2200)).await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.chrome.confetti = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// 祝いの紙吹雪オーバーレイ（`confetti` が true の間だけ全面に色紙が降る）。
    /// 各粒子は同尺の with_animation で落下（delta² の重力）＋末尾フェード。位置は relative で窓非依存。
    fn render_confetti(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.chrome.confetti {
            return None;
        }
        let palette = [
            self.accent(),
            self.theme.ok,
            self.theme.warn,
            self.theme.err,
            project_color(1),
            project_color(3),
            project_color(5),
        ];
        let mut overlay = div().absolute().top_0().left_0().size_full();
        for i in 0..48usize {
            let color = palette[i % palette.len()];
            let x = (i as f32 * 0.618_034) % 1.0; // 黄金比で横に散らす
            let phase = i as f32;
            let wide = i % 2 == 0;
            let particle = div()
                .absolute()
                .w(px(if wide { 7. } else { 5. }))
                .h(px(if i % 3 == 0 { 10. } else { 6. }))
                .rounded(px(2.))
                .bg(color)
                .with_animation(
                    ("confetti", i),
                    Animation::new(std::time::Duration::from_millis(1900)),
                    move |element, delta| {
                        // 粒ごとに落下速度・開始高さをばらす（決定的な擬似乱数）＝帯にならず散らばる。
                        let speed = 0.65 + ((i * 37) % 100) as f32 / 100.0 * 0.7; // 0.65..1.35
                        let start_y = -0.2 - ((i * 53) % 100) as f32 / 100.0 * 0.25;
                        let t = (delta * speed).min(1.0);
                        let fall = start_y + (1.25 - start_y) * t * t; // 上から下へ（重力で加速）
                        let sway = (t * 9.0 + phase).sin() * 0.03; // 横揺れ
                        let opacity = (1.0 - ((t - 0.8) / 0.2).max(0.0)).clamp(0.0, 1.0);
                        element
                            .left(gpui::relative((x + sway).clamp(0.0, 1.0)))
                            .top(gpui::relative(fall))
                            .opacity(opacity)
                    },
                );
            overlay = overlay.child(particle);
        }
        Some(overlay.into_any_element())
    }

    /// 指定コマンドを新しいターミナルタブで実行し、終わったらログインシェルに落ちる（ログイン/導入用）。
    /// 各 CLI の認証はローカルで行う想定なので **ローカルシェル**で走らせる（remote 時も cwd は外す）。
    fn open_command_terminal(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self
            .active_slot()
            .filter(|slot| slot.remote_host.is_none())
            .map(|slot| slot.worktree.root().to_path_buf());
        let shell = Some((
            "/bin/sh".to_string(),
            vec!["-lc".to_string(), format!("{command}; exec \"${{SHELL:-/bin/zsh}}\" -l")],
        ));
        self.chrome.show_bottom = true;
        self.terminal_dock.update(cx, |dock, cx| {
            dock.open_command(TerminalLaunch { cwd, shell }, window, cx)
        });
        cx.notify();
    }

    fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let content = if self.chrome.show_settings {
            // 設定ホーム（中央領域を占有・レール ⚙ で開閉）。第1セクション=Agents。
            let view = self.chrome.settings_view.clone();
            let accent = self.accent();
            view.update(cx, |view, _| view.set_visuals(self.theme.clone(), accent));
            view.into_any_element()
        } else if self.tabs.is_empty() {
            // 初回起動の案内（M13）: 最初の 4 手をキーバッジ付きで。10 分で使い始める導線。
            let hint = |key: &'static str, text: String| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(52.))
                            .flex()
                            .justify_center()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(10.5))
                            .text_color(theme.fg1)
                            .child(key),
                    )
                    .child(div().text_size(px(12.5)).text_color(theme.fg2).child(SharedString::from(text)))
            };
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(px(15.))
                        .text_color(theme.fg1)
                        .pb(px(6.))
                        .child(SharedString::from(i18n::t!("welcome.title"))),
                )
                .child(hint("⌘O", i18n::t!("welcome.open_project")))
                .child(hint("⌘P", i18n::t!("welcome.open_file")))
                .child(hint("⌘⇧A", i18n::t!("welcome.new_thread")))
                .child(hint("⌘⇧P", i18n::t!("welcome.palette")))
                .into_any_element()
        } else {
            let mut panes = div().flex_1().flex().min_h_0().child(self.render_main_pane(cx));
            // 右分割ペイン（あれば仕切り + 2 枚目）。
            if let Some(split) = self.split_editor.clone() {
                panes = panes
                    .child(div().w(px(1.)).flex_none().bg(theme.border))
                    .child(self.render_split_pane(&split, cx));
            }
            panes.into_any_element()
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .bg(theme.bg1)
            // エディタ側を触った → ⌘W の宛先をエディタタブに（Agent 判定を下げる）。
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, _cx| this.agent_active = false))
            .children(self.render_hot_exit_bar(cx))
            .child(content)
            // 下ドック（ターミナル）はエディタ列の下に積む（サイドドックには被らない）。
            .when(self.chrome.show_bottom, |element| element.child(self.render_bottom_dock(cx)))
    }

    fn render_bottom_dock(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        self.terminal_dock.clone()
    }

    fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        // Peacock 相当: statusbar をアクティブプロジェクト色で淡く塗る（窓ごと識別・M13）。
        let accent = self.accent();
        let tint = gpui::Hsla { s: (accent.s + 0.12).min(1.0), a: 0.26, ..accent };
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());
        let remote_host = self.active_slot().and_then(|slot| slot.remote_host.clone());
        let change_count = self.repository.status.len();
        let (cursor, language) = match self.active_editor() {
            Some(editor) => {
                let view = editor.read(cx);
                let (row, column) = view.cursor_display();
                (Some(format!("{row}:{column}")), view.language_label())
            }
            None => (None, None),
        };

        let (errors, warnings) = self.active_diagnostic_counts(cx);
        let error_color = if errors > 0 { theme.err } else { theme.fg2 };
        let warning_color = if warnings > 0 { theme.warn } else { theme.fg2 };
        let left = div()
            .flex()
            .items_center()
            .gap_3()
            // プロジェクト色スウォッチ（footer から色変更・M13）。常に「今の窓の色」が見え、クリックで色ピッカー（⌘K⌘C と同経路）。
            .child(
                div()
                    .id("statusbar-project-color")
                    .size(px(11.))
                    .rounded_full()
                    .bg(self.accent())
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .hover(|style| style.border_color(theme.fg2))
                    .tooltip(Tooltip::text(i18n::t!("cmd.project_color"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            // クリック位置の真上にピッカーを出す（footer から開くので左上へ飛ばさない）。
                            let anchor =
                                gpui::point(event.position.x, event.position.y - px(176.));
                            this.open_color_picker(
                                this.project_sessions.active,
                                anchor,
                                window,
                                cx,
                            )
                        }),
                    ),
            )
            .when_some(remote_host, |element, remote_host| {
                let tooltip = format!("Remote SSH: {remote_host}");
                element.child(
                    div()
                        .id("statusbar-remote-host")
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .max_w(px(180.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(theme.fg0)
                        .child(div().text_color(theme.ok).child("SSH"))
                        .child(remote_host)
                        .tooltip(Tooltip::text(tooltip, theme.clone())),
                )
            })
            // ⎇ ブランチ + 変更件数バッジ。クリックで git パネル（ソース管理）を開閉。
            .when_some(branch, |element, branch| {
                let label = if change_count > 0 {
                    format!("⎇ {branch}  ●{change_count}")
                } else {
                    format!("⎇ {branch}")
                };
                element.child(
                    div()
                        .id("statusbar-branch")
                        .cursor_pointer()
                        .rounded(px(4.))
                        .px(px(4.))
                        .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                        .child(SharedString::from(label))
                        .tooltip(Tooltip::text(i18n::t!("terminal.git_tip"), theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.toggle_git_panel(&ToggleGitPanel, window, cx)
                            }),
                        ),
                )
            })
            // 権限待ちスレッドのドット（M12-5。裏の窓でも気づける）。クリックで Agent パネルへ。
            .when_some(self.waiting_thread.clone(), |element, (name, color)| {
                element.child(
                    div()
                        .id("statusbar-waiting-thread")
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg2))
                        .child(beacon_dot("waiting-pulse", color, true))
                        .child(div().text_color(theme.fg1).child(name))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                this.chrome.show_right = true;
                                this.agent_active = true;
                                cx.notify();
                            }),
                        ),
                )
            })
            .child(
                div()
                    .id("statusbar-diagnostics")
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .rounded(px(4.))
                    .px(px(4.))
                    .child(div().text_color(error_color).child(format!("✗ {errors}")))
                    .child(div().text_color(warning_color).child(format!("▲ {warnings}")))
                    // クリックで診断一覧（ファイル別・M11）。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_diagnostics_panel(&DiagnosticsPanel, window, cx)
                        }),
                    ),
            );

        let right = div()
            .flex()
            .items_center()
            .gap_3()
            // 自動アップデートのチップ（M13）: 新版あり → クリックで更新 → 再起動案内。
            .when_some(self.updater.status.clone(), |element, (info, state)| {
                let (label, clickable) = match state {
                    UpdateState::Available => {
                        (i18n::t!("update.available", "version" => info.version.clone()), true)
                    }
                    UpdateState::Installing => (i18n::t!("update.installing"), false),
                    UpdateState::Ready => (i18n::t!("update.ready"), false),
                };
                element.child(
                    div()
                        .id("update-chip")
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .text_color(self.accent())
                        .when(clickable, |chip| {
                            chip.cursor_pointer()
                                .hover(|style| style.bg(theme.bg2))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| this.install_update(cx)),
                                )
                        })
                        .child(SharedString::from(label)),
                )
            })
            .when_some(cursor, |element, cursor| element.child(SharedString::from(cursor)))
            .child(SharedString::from("UTF-8"))
            .when_some(language, |element, language| element.child(language));

        div()
            .flex()
            .items_center()
            .h(px(STATUSBAR_HEIGHT))
            .px_3()
            .flex_none()
            .bg(tint)
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .child(left)
            .child(div().flex_1())
            .child(right)
    }
}
