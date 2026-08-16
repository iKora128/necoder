use crate::workspace::*;

impl Workspace {
    pub(crate) fn open_file_from_launcher(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(i18n::t!("launcher.open_file"))),
        });
        cx.spawn(async move |workspace, cx| {
            let result = receiver.await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Ok(Ok(Some(paths))) = result {
                    if let Some(path) = paths.into_iter().next() {
                        workspace.pending_navigation = Some((path, 0, 0));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(crate) fn rail_icon(
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

    pub(crate) fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.project_sessions.active;
        let accent = self.accent();
        let rail = settings::get(cx).rail; // アイコンの表示/非表示（settings 反応）
                                           // 他プロジェクトの承認待ち・完了に気づけるよう、各スロットへ最重要スレッドの状態ドットを出す。
                                           // Working は左レールでは表示しない（動かない小さな loading 表示に見えるため）。実行中は
                                           // herd / titlebar beacon / statusbar の情報量がある面へ集約する。
                                           // （ARCHITECTURE §5「レールのドットが他 project 分を担う」・#）。cx を跨いで借用しないよう
                                           // root → (スレッド色, 状態) を先に所有データへ畳んでおく。
        let rail_signals: std::collections::HashMap<
            std::path::PathBuf,
            (Hsla, agent_panel::ThreadActivity),
        > = cx
            .try_global::<agent_panel::RunningRegistry>()
            .map(|registry| {
                registry
                    .0
                    .iter()
                    .filter_map(|(root, rows)| {
                        rows.iter()
                            .filter(|row| {
                                row.2.is_signal()
                                    && !matches!(row.2, agent_panel::ThreadActivity::Working)
                            })
                            .max_by_key(|row| row.2.urgency())
                            .map(|(_, color, activity)| (root.clone(), (*color, *activity)))
                    })
                    .collect()
            })
            .unwrap_or_default();
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
            .children(
                {
                    // レール = プロジェクト（リポジトリ）単位。**Task worktree はレールに載せない**
                    // （同じリポジトリが色違いで並ぶと「色による方向感覚」が壊れる・2026-07-24 ユーザー指摘）。
                    // Task はレールでなく編隊モード（セル/herd）に住む。アクティブが Task slot の時は
                    // 同じリポジトリの Integration 枠を点灯させる（今どのプロジェクトに居るかは保つ）。
                    let active_slot_is_task = self
                        .project_sessions
                        .projects
                        .get(active)
                        .is_some_and(|slot| !slot.task_space.is_integration());
                    let active_repository = self
                        .project_sessions
                        .projects
                        .get(active)
                        .map(|slot| slot.task_space.repository_id.clone());
                    self.project_sessions
                        .projects
                        .iter()
                        .enumerate()
                        .filter(|(_, slot)| slot.task_space.is_integration())
                        .map(move |(index, slot)| {
                            (index, slot, active_slot_is_task, active_repository.clone())
                        })
                }
                .map(|(index, slot, active_slot_is_task, active_repository)| {
                    let color = slot.color;
                    let is_active = index == active
                        || (active_slot_is_task
                            && active_repository.as_deref()
                                == Some(slot.task_space.repository_id.as_str()));
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
                        .when(slot.remote_host.is_some(), |element| {
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
                                    .child(
                                        svg()
                                            .path("icons/server.svg")
                                            .size(px(8.))
                                            .text_color(color),
                                    ),
                            )
                        })
                        // 状態ドット（右上・絶対配置）。他プロジェクトが承認待ち/完了(未確認)なら灯る。
                        // Working の loading 表示は左レールには重複させない。
                        // （リモートバッジは右下なので衝突しない）。色はスレッド識別色・状態は形と動きで（§1.3）。
                        .when_some(
                            rail_signals.get(slot.worktree.root()).copied(),
                            |element, (dot_color, activity)| {
                                element.child(div().absolute().top(px(-2.)).right(px(-2.)).child(
                                    agent_panel::activity_dot(
                                        ("rail-activity", index),
                                        7.0,
                                        dot_color,
                                        activity,
                                    ),
                                ))
                            },
                        )
                        .tooltip(Tooltip::text(name, theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                // クリック = 切替（従来どおり down で確定）。同時にドラッグ追跡を開始し、
                                // 窓の外で離せば擬似 tear-off = その位置に新窓（rail.rs）。
                                this.chrome.rail_drag = Some((index, event.position, false));
                                this.switch_project(index, window, cx)
                            }),
                        )
                }),
            )
            // ＋ = 統一オープン（フォルダ/ファイル/リモート + 最近を1枚のファジー面に・open_launcher）
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
                    .hover(|style| {
                        style
                            .text_color(theme.fg0)
                            .border_color(theme.fg2)
                            .bg(theme.bg2)
                    })
                    .tooltip(Tooltip::text(i18n::t!("rail.add_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.open_launcher(window, cx)),
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
                            && !self.chrome.show_herd
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
                            if this.todo_panel.read(cx).open
                                || this.git_panel_open(cx)
                                || this.chrome.show_herd
                            {
                                this.todo_panel
                                    .update(cx, |panel, cx| panel.set_open(false, cx));
                                this.git_panel
                                    .update(cx, |panel, cx| panel.set_open(false, cx));
                                this.chrome.show_herd = false;
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
                        if self.search_panel.is_some() {
                            accent
                        } else {
                            theme.fg2
                        },
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_project_search(&ProjectSearch, window, cx)
                        }),
                    ),
                )
            })
            // 編隊モードの入口は titlebar 右上の「Multi Agent」トグルへ移設（レール ⚡ は廃止・M14）。
            .when(rail.git, |element| {
                element.child(
                    self.rail_icon(
                        "rail-git",
                        "icons/git-branch.svg",
                        i18n::t!("rail.git"),
                        if self.git_panel_open(cx) {
                            accent
                        } else {
                            theme.fg2
                        },
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.toggle_git_panel(&ToggleGitPanel, window, cx)
                        }),
                    ),
                )
            })
            .when(rail.todos, |element| {
                // Todo ボード（.shirushi/todos.md・M12-10）。表示中（アクティブ）はプロジェクト色。
                let color = if self.todo_panel.read(cx).open {
                    accent
                } else {
                    theme.fg2
                };
                element.child(
                    self.rail_icon(
                        "rail-todos",
                        "icons/square-check.svg",
                        i18n::t!("rail.todos"),
                        color,
                    )
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
                        if self.chrome.show_right {
                            accent
                        } else {
                            theme.fg2
                        },
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
                        // 編隊では下段のタブが「ターミナル」なら点灯（solo は下ドックの開閉）。
                        if if self.chrome.fleet_mode {
                            self.chrome.fleet_bottom_view == FleetBottomView::Terminal
                        } else {
                            self.chrome.show_bottom
                        } {
                            accent
                        } else {
                            theme.fg2
                        },
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.toggle_terminal(&ToggleTerminal, window, cx)
                        }),
                    ),
                )
            })
            .when(rail.remote, |element| {
                // リモート SSH（~/.ssh/config → ワンクリック接続・#2）。アクティブがリモートなら色付き。
                let is_remote_active = self
                    .active_slot()
                    .map(|slot| slot.remote_host.is_some())
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
                    if self.chrome.show_settings {
                        accent
                    } else {
                        theme.fg2
                    },
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.chrome.show_settings = !this.chrome.show_settings;
                        if this.chrome.show_settings {
                            this.chrome
                                .settings_view
                                .update(cx, |view, cx| view.refresh_availability(cx));
                        }
                        cx.notify();
                    }),
                ),
            )
            .child(div().h(px(6.)))
    }
}
