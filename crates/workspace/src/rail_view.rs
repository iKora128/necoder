impl Workspace {
    fn rail_icon(
        &self,
        id: &'static str,
        icon: &'static str,
        tooltip: impl Into<SharedString>,
        color: Hsla,
    ) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .size(px(30.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            .child(svg().path(icon).size(px(17.)).text_color(color))
            .tooltip(Tooltip::text(tooltip, theme.clone()))
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.project_sessions.active;
        let accent = self.accent();
        let rail = settings::get(cx).rail; // アイコンの表示/非表示（settings 反応）
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .pt_2()
            .children(self.project_sessions.projects.iter().enumerate().map(|(index, slot)| {
                let color = slot.color;
                let is_active = index == active;
                let monogram = slot
                    .icon
                    .as_ref()
                    .map(|icon| icon.to_string())
                    .unwrap_or_else(|| slot.name.chars().next().unwrap_or('•').to_string());
                let name = slot.name.clone();
                div()
                    .id(("rail-project", index))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.open_rail_menu(index, event.position, cx);
                        }),
                    )
                    .size(px(30.))
                    .relative() // リモートバッジ（絶対配置）の基準
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.fg0)
                    .bg(color.alpha(0.14))
                    .border_2()
                    .border_color(if is_active { color } else { color.alpha(0.35) })
                    .cursor_pointer()
                    // 非アクティブは hover で色が濃くなる＝クリックできる合図（Zed の気持ちよさ）
                    .hover(|style| style.bg(color.alpha(0.24)).border_color(color))
                    .child(monogram)
                    .when(slot.worktree.host().is_remote(), |element| {
                        // リモート slot の見分け（#2）: 右下に server バッジ（SSH 接続先の目印）。
                        element.child(
                            div()
                                .absolute()
                                .bottom(px(-3.))
                                .right(px(-3.))
                                .size(px(13.))
                                .rounded_full()
                                .bg(theme.bg0)
                                .border_1()
                                .border_color(color)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(svg().path("icons/server.svg").size(px(8.)).text_color(color)),
                        )
                    })
                    .tooltip(Tooltip::text(name, theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.switch_project(index, window, cx)),
                    )
            }))
            // ＋ = フォルダ選択ダイアログ → このウィンドウのレールへ追加（多重起動はガード済み）
            .child(
                div()
                    .id("rail-add")
                    .size(px(30.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.fg2)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .child("＋")
                    .hover(|style| style.text_color(theme.fg0).border_color(theme.fg2).bg(theme.bg2))
                    .tooltip(Tooltip::text(i18n::t!("rail.add_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.add_project_via_dialog(cx)),
                    ),
            )
            .child(div().flex_1())
            // ── アクティビティアイコン（settings.rail で個別に表示/非表示・Lucide SVG）──
            .when(rail.explorer, |element| {
                element.child(
                    self.rail_icon(
                        "rail-explorer",
                        "icons/panel-left.svg",
                        i18n::t!("rail.explorer"),
                        // アクティブ（左ドックがエクスプローラ表示）ならプロジェクト色・でなければ淡色（VSCode 風）。
                        if self.chrome.show_left
                            && !self.todo_panel.read(cx).open
                            && !self.git_panel_open(cx)
                        {
                            accent
                        } else {
                            theme.fg2
                        },
                    )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                // エクスプローラ（ファイルブラウザ）はレールの既定ビュー。
                                // Todo/git が出ていれば**それをクリアして**エクスプローラへ戻す
                                // （旧: show_left トグルのみ → Todo が居座り「開くと Todo」問題）。
                                // 既にエクスプローラなら従来どおり表示トグル。
                                if this.todo_panel.read(cx).open || this.git_panel_open(cx) {
                                    this.todo_panel.update(cx, |panel, cx| panel.set_open(false, cx));
                                    this.git_panel.update(cx, |panel, cx| panel.set_open(false, cx));
                                    this.chrome.show_left = true;
                                } else {
                                    this.chrome.show_left = !this.chrome.show_left;
                                }
                                cx.notify();
                            }),
                        ),
                )
            })
            .when(rail.search, |element| {
                element.child(
                    self.rail_icon(
                        "rail-search",
                        "icons/search.svg",
                        i18n::t!("rail.search"),
                        if self.search_panel.is_some() { accent } else { theme.fg2 },
                    ).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.open_project_search(&ProjectSearch, window, cx)),
                    ),
                )
            })
            .when(rail.git, |element| {
                element.child(
                    self.rail_icon(
                        "rail-git",
                        "icons/git-branch.svg",
                        i18n::t!("rail.git"),
                        if self.git_panel_open(cx) { accent } else { theme.fg2 },
                    ).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.toggle_git_panel(&ToggleGitPanel, window, cx)),
                    ),
                )
            })
            .when(rail.todos, |element| {
                // Todo ボード（.shirushi/todos.md・M12-10）。表示中（アクティブ）はプロジェクト色。
                let color = if self.todo_panel.read(cx).open { accent } else { theme.fg2 };
                element.child(
                    self.rail_icon("rail-todos", "icons/square-check.svg", i18n::t!("rail.todos"), color)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.toggle_todo_board(&ToggleTodoBoard, window, cx)
                            }),
                        ),
                )
            })
            .when(rail.agent, |element| {
                element.child(
                    self.rail_icon(
                        "rail-agent",
                        "icons/sparkles.svg",
                        i18n::t!("rail.agent"),
                        // AI ドック（右）表示中はプロジェクト色・畳んでいれば淡色。
                        if self.chrome.show_right { accent } else { theme.fg2 },
                    )
                        // 新規スレッドではなく Agent パネルの開閉トグル（他のアクティビティアイコンと同じ所作）。
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.toggle_dock(Dock::Right, cx)),
                        ),
                )
            })
            .when(rail.terminal, |element| {
                element.child(
                    self.rail_icon(
                        "rail-terminal",
                        "icons/square-terminal.svg",
                        i18n::t!("rail.terminal"),
                        if self.chrome.show_bottom { accent } else { theme.fg2 },
                    )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.toggle_terminal(&ToggleTerminal, window, cx)),
                        ),
                )
            })
            .when(rail.remote, |element| {
                // リモート SSH（~/.ssh/config → ワンクリック接続・#2）。アクティブがリモートなら色付き。
                let is_remote_active = self
                    .active_slot()
                    .map(|slot| slot.worktree.host().is_remote())
                    .unwrap_or(false);
                element.child(
                    self.rail_icon(
                        "rail-remote",
                        "icons/server.svg",
                        i18n::t!("rail.remote"),
                        if is_remote_active { accent } else { theme.fg2 },
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_ssh_host_picker(&RemoteSsh, window, cx)
                        }),
                    ),
                )
            })
            // ⚙ 設定ホーム（中央に開く。Agents セットアップの入口・常時表示）。
            .child(
                self.rail_icon(
                    "rail-settings",
                    "icons/settings.svg",
                    i18n::t!("rail.settings"),
                    if self.chrome.show_settings { accent } else { theme.fg2 },
                )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            this.chrome.show_settings = !this.chrome.show_settings;
                            if this.chrome.show_settings {
                                this.chrome.settings_view.update(cx, |view, cx| {
                                    view.refresh_availability(cx)
                                });
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(div().h(px(6.)))
    }
}
