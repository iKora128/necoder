use crate::workspace::*;

impl Workspace {
    pub(crate) fn on_settings_view_event(
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

    pub(crate) fn accent(&self) -> Hsla {
        self.active_slot()
            .map(|slot| slot.color)
            .unwrap_or_else(|| project_color(0))
    }

    // ── titlebar（UI-SPEC §3） ──

    pub(crate) fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        // Peacock 相当: titlebar をアクティブプロジェクト色で淡く塗る（窓ごと識別・M13）。
        let accent = self.accent();
        let tint = gpui::Hsla {
            s: (accent.s + 0.12).min(1.0),
            a: 0.26,
            ..accent
        };
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
            .on_mouse_down_out(
                cx.listener(|this, _, _window, _cx| this.chrome.should_move_window = false),
            )
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
            // Multi Agent（編隊）モードのトグル（右上・UI-SPEC §11）。通常 ⇄ 編隊ビューを切替。
            .child(self.render_fleet_toggle(cx))
            // 実行中スレッドの beacon（窓上部から常に見える＝方向感覚の核・UI-SPEC §3）
            .child(self.render_beacons(cx))
            // リモート SSH で開く（M13・GUI 導線。~/.ssh/config のエイリアス/鍵/ProxyJump がそのまま効く）
            .child(
                self.rail_icon(
                    "titlebar-ssh",
                    "icons/server.svg",
                    i18n::t!("ssh.button_tip"),
                    if self.overlays.ssh_connecting {
                        self.accent()
                    } else {
                        theme.fg2
                    },
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

    /// Multi Agent（編隊）モードのトグル（titlebar 右上・M14）。ON で全画面が編隊ビュー
    /// （herd + 系譜グラフ + N分割グリッド + ニュース）に。ON の間はプロジェクト色でハイライト。
    pub(crate) fn render_fleet_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let on = self.chrome.fleet_mode;
        let accent = self.accent();
        div()
            .id("titlebar-fleet")
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(24.))
            .px(px(9.))
            .rounded(px(6.))
            .border_1()
            .border_color(if on { accent } else { theme.border })
            .when(on, |element| element.bg(accent.alpha(0.16)))
            .text_color(if on { theme.fg0 } else { theme.fg2 })
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2).text_color(theme.fg1))
            .child(
                svg()
                    .path("icons/layout-grid.svg")
                    .size(px(13.))
                    .flex_none()
                    .text_color(if on { accent } else { theme.fg2 }),
            )
            .child(div().text_size(px(11.)).child(i18n::t!("titlebar.fleet")))
            .tooltip(Tooltip::text(i18n::t!("rail.fleet"), theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.toggle_fleet_mode(&ToggleFleet, window, cx)
                }),
            )
    }

    /// titlebar の beacon 列（アクティブプロジェクトのスレッド。実行中は色濃く・停止中は淡く）。
    pub(crate) fn render_beacons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let beacons = self.agent_panel.read(cx).beacons();
        div()
            .flex()
            .items_center()
            .gap_3()
            .mr_2()
            .children(
                beacons
                    .into_iter()
                    .enumerate()
                    .map(|(index, (name, color, activity))| {
                        let label = format!("{name} — {}", activity_label(activity));
                        div()
                            .id(("beacon-item", index))
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(11.))
                            .text_color(if activity.is_signal() {
                                theme.fg1
                            } else {
                                theme.fg2
                            })
                            .child(agent_panel::activity_dot(
                                ("beacon", index),
                                8.0,
                                color,
                                activity,
                            ))
                            .child(name)
                            .tooltip(Tooltip::text(label, theme.clone()))
                    }),
            )
    }

    /// プロジェクトピル: 枠 + 「名前 ▾」+「⎇ branch」。名前クリックで ⌘O。
    /// プロジェクト色はレール/キャレット等の許可箇所に集約（左縁チップは廃止・2026-07-17）。
    pub(crate) fn render_project_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let name = self
            .active_slot()
            .map(|slot| slot.name.clone())
            .unwrap_or_else(|| SharedString::from("—"));
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());

        let mut inner = div()
            .flex()
            .items_center()
            .gap(px(11.))
            .py(px(4.))
            .px(px(11.))
            .child(
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
                    .tooltip(Tooltip::text(
                        i18n::t!("git.branch_menu_tip"),
                        theme.clone(),
                    ))
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
    pub(crate) fn dock_button(
        &self,
        dock: Dock,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            Dock::Bottom => frame
                .flex_col()
                .justify_end()
                .child(strip().h(px(3.5)).w_full()),
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
    pub(crate) fn render_main_tabstrip(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                let name_color = status
                    .map(|status| Self::git_tint(&theme, status))
                    .unwrap_or(theme.fg0);
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
                    .child(
                        div()
                            .h(px(2.))
                            .w_full()
                            .bg(if is_active { accent } else { theme.bg0 }),
                    )
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
                                    .tooltip(Tooltip::text(
                                        i18n::t!("tabs.close_tip"),
                                        theme.clone(),
                                    ))
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
                            this.chrome.show_settings = false; // タブを押したら設定ホームは退く
                            this.select_tab(index, window, cx);
                        }),
                    )
                    // Chrome 風ドラッグ並べ替え: タブを掴んで別タブ上で離すと順序が入れ替わる。
                    .on_drag(
                        DraggedEditorTab {
                            index,
                            name: drag_name,
                            theme: drag_theme,
                        },
                        |dragged, _offset, _window, cx| cx.new(|_| dragged.clone()),
                    )
                    .drag_over::<DraggedEditorTab>(move |style, _dragged, _window, _cx| {
                        style.bg(drop_highlight)
                    })
                    .on_drop(
                        cx.listener(move |this, dragged: &DraggedEditorTab, _window, cx| {
                            this.move_tab(dragged.index, index, cx);
                        }),
                    )
            }))
            .child(div().flex_1())
    }

    /// 右分割ペインのタブ列（単一比較ビュー。× = 分割を閉じる）。
    pub(crate) fn render_split_tabstrip(
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
                                    .tooltip(Tooltip::text(
                                        i18n::t!("tabs.close_split_tip"),
                                        theme.clone(),
                                    ))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.close_split(window, cx)
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(div().flex_1())
    }

    pub(crate) fn render_breadcrumb(
        &self,
        editor: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let path = editor.read(cx).buffer().path().map(Path::to_path_buf);
        let root = self
            .active_slot()
            .map(|slot| slot.worktree.root().to_path_buf());
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
    pub(crate) fn render_buffer_search_bar(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
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
            let text_color = if text.is_empty() {
                theme.fg2
            } else {
                theme.fg0
            };
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
                    .tooltip(Tooltip::text(
                        i18n::t!("search.toggle_replace"),
                        theme.clone(),
                    ))
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
                chip(
                    "bsearch-prev",
                    "‹",
                    false,
                    SharedString::from(i18n::t!("search.previous")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.step_buffer_match(-1, cx)),
                ),
            )
            .child(
                chip(
                    "bsearch-next",
                    "›",
                    false,
                    SharedString::from(i18n::t!("search.next")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.step_buffer_match(1, cx)),
                ),
            )
            .child(
                chip(
                    "bsearch-close",
                    "×",
                    false,
                    SharedString::from(i18n::t!("search.close")),
                )
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
                .shadow(vec![gpui::BoxShadow::new(
                    px(0.),
                    px(6.),
                    gpui::hsla(0., 0., 0., 0.35),
                )
                .blur_radius(px(18.))])
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
    pub(crate) fn render_main_pane(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
                    .child(
                        editor
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    )
                    .children(self.render_buffer_search_bar(cx)),
            )
            .into_any_element()
    }

    /// 右分割ペイン（単一タブ + パンくず + 本体）。
    pub(crate) fn render_split_pane(
        &self,
        editor: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(self.render_split_tabstrip(editor, cx))
            .child(self.render_breadcrumb(editor, cx))
            .child(
                div().flex_1().overflow_hidden().child(
                    editor
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
            .into_any_element()
    }

    /// 祝いの紙吹雪を降らせる（~2.2s で自動的に止める）。
    pub(crate) fn celebrate_confetti(&mut self, cx: &mut Context<Self>) {
        self.chrome.confetti = true;
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(2200))
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.chrome.confetti = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// 祝いの紙吹雪オーバーレイ（`confetti` が true の間だけ全面に色紙が降る）。
    /// 各粒子は同尺の with_animation で落下（delta² の重力）＋末尾フェード。位置は relative で窓非依存。
    pub(crate) fn render_confetti(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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
    pub(crate) fn open_command_terminal(
        &mut self,
        command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self
            .active_slot()
            .filter(|slot| slot.remote_host.is_none())
            .map(|slot| slot.worktree.root().to_path_buf());
        let shell = Some((
            "/bin/sh".to_string(),
            vec![
                "-lc".to_string(),
                format!("{command}; exec \"${{SHELL:-/bin/zsh}}\" -l"),
            ],
        ));
        self.chrome.show_bottom = true;
        self.terminal_dock.update(cx, |dock, cx| {
            dock.open_command(TerminalLaunch { cwd, shell }, window, cx)
        });
        cx.notify();
    }

    pub(crate) fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let content = if self.chrome.show_settings {
            // 設定ホーム（レール ⚙ で開閉）。第1セクション=Agents。
            let view = self.chrome.settings_view.clone();
            let accent = self.accent();
            view.update(cx, |view, _| view.set_visuals(self.theme.clone(), accent));
            let body = div().flex_1().min_h_0().child(view);
            if self.tabs.is_empty() {
                body.into_any_element()
            } else {
                // 設定を開いても**開いているファイルタブは残す**（タブを押せば設定が退いてそのファイルへ戻る）。
                // 以前は中央を丸ごと settings に差し替えていて「開いた瞬間ファイルが消える」体感だった（UX 修正）。
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .min_w_0()
                    .child(self.render_main_tabstrip(cx))
                    .child(body)
                    .into_any_element()
            }
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
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(text)),
                    )
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
            let mut panes = div()
                .flex_1()
                .flex()
                .min_h_0()
                .child(self.render_main_pane(cx));
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.agent_active = false),
            )
            .children(self.render_hot_exit_bar(cx))
            .child(content)
            // 下ドック（ターミナル）はエディタ列の下に積む（サイドドックには被らない）。
            .when(self.chrome.show_bottom, |element| {
                element.child(self.render_bottom_dock(cx))
            })
    }

    /// AI 全画面時の中央列（2026-08-08 改訂）。**エディタの代わりに Agent パネルを中央へ据える**だけで、
    /// 下ドック（ターミナル）は `render_center` と同じく列の下に積む（起動していれば残る・本人要望）。
    /// 左ドックと右ドックの取り回しは呼び出し側（`Workspace::render`）が各自の ON/OFF で決める。
    pub(crate) fn render_agent_full_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .bg(theme.bg1)
            .child(
                div().flex_1().min_h_0().min_w_0().child(
                    self.agent_panel
                        .clone()
                        .cached(StyleRefinement::default().flex().flex_col().size_full()),
                ),
            )
            .when(self.chrome.show_bottom, |element| {
                element.child(self.render_bottom_dock(cx))
            })
    }

    /// solo の下ドック（ターミナル）。編隊の下段と同じ `bottom_height` を上縁ドラッグで共有する
    /// （2026-07-27。以前は TerminalDock 側の固定 240px で、伸ばせなかった）。
    pub(crate) fn render_bottom_dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(self.chrome.bottom_height))
            .flex()
            .flex_col()
            .child(self.render_bottom_resize_handle(cx))
            .child(
                self.terminal_dock
                    .clone()
                    .cached(StyleRefinement::default().flex().flex_col().size_full()),
            )
    }

    /// フッター中央の**常設ロールアップ**（herdr の状態集約・#）。ウィンドウのレールに載る全プロジェクトの
    /// スレッド状態を `RunningRegistry` から集計し「N 実行 · M 承認待ち · K 完了」を常時表示。最重要
    /// （Blocked>Working>Done）の代表スレッドを状態ドット＋名前で出し、click で該当プロジェクト＋Agent へ。
    /// ニュースティッカーではない（今どうなっているかの一覧性を優先）。0 件なら空スペーサー。
    /// フッターのニュース欄に出す稼働シグナル（実行中/承認待ち/完了未確認）を全プロジェクトから集める。
    /// 緊急度（Blocked>Working>Done）降順で安定ソート＝順送りの並びと先頭（初期代表）を決める。
    /// render（表示）と回転ティッカー（自停止判定）の両方が使う。
    fn activity_signals(
        &self,
        cx: &App,
    ) -> Vec<(usize, SharedString, Hsla, agent_panel::ThreadActivity)> {
        let registry = cx.try_global::<agent_panel::RunningRegistry>();
        let mut signals: Vec<(usize, SharedString, Hsla, agent_panel::ThreadActivity)> = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if let Some(rows) = registry.and_then(|reg| reg.0.get(slot.worktree.root())) {
                for (name, color, activity) in rows {
                    if activity.is_signal() {
                        signals.push((index, name.clone(), *color, *activity));
                    }
                }
            }
        }
        signals.sort_by_key(|signal| std::cmp::Reverse(signal.3.urgency()));
        signals
    }

    /// ニュース欄の順送りを回す（稼働シグナルが 2 件以上ある間だけ ~3.2 秒ごとに `rollup_index` を進める）。
    /// 多重起動は `rollup_ticker` で防ぎ、2 件未満になったら次 tick で自停止する（idle 予算を守る＝
    /// `fleet_clock` と同型）。トップの `render` から毎フレーム呼ばれるが、稼働中は即 return する。
    pub(crate) fn ensure_rollup_ticker(&mut self, cx: &mut Context<Self>) {
        if self.chrome.rollup_ticker || !self.window_active || self.activity_signals(cx).len() < 2 {
            return;
        }
        self.chrome.rollup_ticker = true;
        cx.spawn(async move |workspace, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(3200))
                .await;
            let keep_running = workspace.update(cx, |workspace, cx| {
                if workspace.window_active && workspace.activity_signals(cx).len() >= 2 {
                    workspace.chrome.rollup_index = workspace.chrome.rollup_index.wrapping_add(1);
                    cx.notify();
                    true
                } else {
                    workspace.chrome.rollup_ticker = false;
                    false
                }
            });
            if !matches!(keep_running, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    fn render_activity_rollup(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use agent_panel::ThreadActivity;
        let theme = self.theme.clone();
        let signals = self.activity_signals(cx);
        let container = div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .h_full();
        if signals.is_empty() {
            return container.into_any_element(); // 空＝ただのスペーサー（従来と同じ）
        }
        // 右側の集計サマリ（従来どおり「N 実行 · M 承認待ち · K 完了」）。
        let count = |want: ThreadActivity| {
            signals
                .iter()
                .filter(|signal| std::mem::discriminant(&signal.3) == std::mem::discriminant(&want))
                .count()
        };
        let blocked = count(ThreadActivity::Blocked);
        let working = count(ThreadActivity::Working);
        let done = count(ThreadActivity::Done { interrupted: false });
        let mut segments: Vec<String> = Vec::new();
        if blocked > 0 {
            segments.push(i18n::t!("agent.rollup_blocked", "n" => blocked));
        }
        if working > 0 {
            segments.push(i18n::t!("agent.rollup_working", "n" => working));
        }
        if done > 0 {
            segments.push(i18n::t!("agent.rollup_done", "n" => done));
        }
        let summary = SharedString::from(segments.join("  ·  "));

        // 順送りで今見せる 1 件（`rollup_index` を件数で mod。ティッカーが 3.2 秒ごとに進める）。
        let total = signals.len();
        let position = self.chrome.rollup_index % total;
        let (index, name, color, activity) = signals[position].clone();
        // 中身（●名前）は切替のたびに淡くフェードイン（id が rollup_index で変わる＝新アニメ）。
        // 1回限りなので切替の 280ms 以外は再描画ゼロ（idle 予算を守る）。クリック判定は包まない
        // 外側 div に持たせ、アニメは内容だけに掛ける（インタラクションを壊さない）。
        let content = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(agent_panel::activity_dot(
                "rollup-rep",
                8.0,
                color,
                activity,
            ))
            .child(div().text_color(theme.fg1).child(name))
            .with_animation(
                ("rollup-fade", self.chrome.rollup_index),
                Animation::new(std::time::Duration::from_millis(280)),
                |element, delta| element.opacity(0.25 + 0.75 * delta),
            );
        let current = div()
            .id("statusbar-activity-rollup")
            .flex()
            .items_center()
            .px(px(8.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.switch_project(index, window, cx);
                    this.chrome.show_right = true;
                    this.agent_active = true;
                    cx.notify();
                }),
            )
            .child(content);

        container
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(current)
                    // 2 件以上なら位置ドット（●=今 / ·=他）で「順送り中・全部で N 件」を示す。
                    .when(total >= 2, |element| {
                        let mut dots = div().flex().items_center().gap(px(3.));
                        for slot in 0..total {
                            let on = slot == position;
                            dots = dots.child(
                                div()
                                    .size(px(if on { 5. } else { 4. }))
                                    .rounded_full()
                                    .bg(if on { theme.fg1 } else { theme.fg2.alpha(0.4) }),
                            );
                        }
                        element.child(dots)
                    })
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(summary),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        // Peacock 相当: statusbar をアクティブプロジェクト色で淡く塗る（窓ごと識別・M13）。
        let accent = self.accent();
        let tint = gpui::Hsla {
            s: (accent.s + 0.12).min(1.0),
            a: 0.26,
            ..accent
        };
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
        // 承認待ち signal は1Hz時計だけで反転。マスコットの5/10fps時計とは共有しない。
        let attention_bright = self.visual_tick % 2 == 0;
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
                            let anchor = gpui::point(event.position.x, event.position.y - px(176.));
                            this.open_color_picker(this.project_sessions.active, anchor, window, cx)
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
                        .child(beacon_dot("waiting-pulse", color, attention_bright))
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
                    .child(
                        div()
                            .text_color(warning_color)
                            .child(format!("▲ {warnings}")),
                    )
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
            // 前回クラッシュの通知チップ（M13）: クリックでログ抜粋つきのバグ報告 Issue を開く。
            // 色は theme.warn（診断 ▲ と同じ警告色 = 識別色は使わない）。
            .when_some(self.notifications.crash_notice.clone(), |element, _log| {
                element.child(
                    div()
                        .id("crash-chip")
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .text_color(theme.warn)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg2))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.report_crash(cx)),
                        )
                        .child(SharedString::from(i18n::t!("crash.notice"))),
                )
            })
            // 自動アップデートのチップ（M13）: 新版あり → クリックで更新 → 再起動案内。
            .when_some(self.updater.status.clone(), |element, (info, state)| {
                let (label, clickable) = match state {
                    UpdateState::Available => (
                        i18n::t!("update.available", "version" => info.version.clone()),
                        true,
                    ),
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
            .when_some(cursor, |element, cursor| {
                element.child(SharedString::from(cursor))
            })
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
            .child(self.render_activity_rollup(cx)) // 中央＝状態の常設ロールアップ（herdr 本来の形）
            .child(right)
    }

    // ── 自動アップデート（M13）: statusbar チップの中身 ──

    /// 起動しばらく後に GitHub Releases を確認する（背景・失敗は静かに無視）。
    /// スクショ/プローブ実行時と `SHIRUSHI_NO_UPDATE_CHECK` ではネットへ出ない。
    pub(crate) fn schedule_update_check(&self, cx: &mut Context<Self>) {
        if cfg!(test)
            || std::env::var_os("SHIRUSHI_NO_UPDATE_CHECK").is_some()
            || std::env::var_os("SHIRUSHI_SCREENSHOT").is_some()
        {
            return;
        }
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(90))
                .await;
            let found = cx
                .background_executor()
                .spawn(async move { updater::check_for_update(env!("CARGO_PKG_VERSION")) })
                .await;
            if let Some(info) = found {
                let _ = workspace.update(cx, |workspace, cx| {
                    workspace.updater.status = Some((info, UpdateState::Available));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // ── メニューバー連携（M13: 設定を開く / About） ──

    /// ⌘, / メニュー「設定…」。rail の ⚙ トグルと違い、常に「開く」（メニューの意味論）。
    pub(crate) fn open_settings_action(
        &mut self,
        _: &OpenSettings,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chrome.show_settings = true;
        self.chrome
            .settings_view
            .update(cx, |view, cx| view.refresh_availability(cx));
        cx.notify();
    }

    /// メニュー「Shirushi について」。バージョン表記のトースト（About パネルの最小版）。
    pub(crate) fn about_action(&mut self, _: &About, _window: &mut Window, cx: &mut Context<Self>) {
        let accent = self.accent();
        self.push_toast(
            SharedString::from(format!(
                "Shirushi v{} — AGPL-3.0 · shirushi.ai",
                env!("CARGO_PKG_VERSION")
            )),
            accent,
            cx,
        );
    }

    // ── クラッシュ通知 + バグ報告（M13: panic hook → ログ → GitHub Issue） ──

    /// 起動時に前回クラッシュの pending マーカーを 1 回だけ消費してチップを出す（背景）。
    /// offscreen 撮影ではユーザーの実マーカーを消費しない（SHIRUSHI_CRASH_DIR 指定時のみ読む）。
    pub(crate) fn check_crash_notice(&mut self, cx: &mut Context<Self>) {
        // プローブ: チップ描画の offscreen 検証用（debug のみ・実ログ不要）。
        #[cfg(debug_assertions)]
        if std::env::var_os("SHIRUSHI_CRASH_PROBE").is_some() {
            self.notifications.crash_notice = Some(PathBuf::from("/tmp/shirushi-crash-probe.log"));
            cx.notify();
            return;
        }
        if cfg!(test)
            || (std::env::var_os("SHIRUSHI_SCREENSHOT").is_some()
                && std::env::var_os("SHIRUSHI_CRASH_DIR").is_none())
        {
            return;
        }
        cx.spawn(async move |workspace, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { crate::crash::take_pending_crash() })
                .await;
            if let Some(log_path) = found {
                let _ = workspace.update(cx, |workspace, cx| {
                    workspace.notifications.crash_notice = Some(log_path);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// クラッシュチップのクリック: ログ抜粋つきのバグ報告 Issue を開き、チップを消す。
    pub(crate) fn report_crash(&mut self, cx: &mut Context<Self>) {
        let Some(log_path) = self.notifications.crash_notice.take() else {
            return;
        };
        cx.notify();
        self.open_bug_report(Some(log_path), cx);
    }

    /// ⌘⇧P「ヘルプ: バグを報告」: 環境情報だけ事前記入した Issue を開く。
    pub(crate) fn report_bug_action(
        &mut self,
        _: &ReportBug,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_bug_report(None, cx);
    }

    /// new issue URL を組んでブラウザで開く（sw_vers・ログ読みがあるので背景で）。
    fn open_bug_report(&mut self, crash_log: Option<PathBuf>, cx: &mut Context<Self>) {
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let url = crate::crash::bug_report_url(crash_log.as_deref());
                    crate::crash::open_url(&url)
                })
                .await;
            if let Err(error) = result {
                let _ = workspace.update(cx, |workspace, cx| {
                    let accent = workspace.accent();
                    workspace.push_toast(SharedString::from(format!("{error:#}")), accent, cx);
                });
            }
        })
        .detach();
    }

    /// statusbar チップのクリック: ダウンロード → 署名検証 → 差し替え（背景）。
    pub(crate) fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some((info, state)) = self.updater.status.clone() else {
            return;
        };
        if state != UpdateState::Available {
            return;
        }
        self.updater.status = Some((info.clone(), UpdateState::Installing));
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater::download_and_install(&info).map(|_| info) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match result {
                Ok(info) => {
                    workspace.updater.status = Some((info, UpdateState::Ready));
                    cx.notify();
                }
                Err(error) => {
                    workspace.updater.status = None;
                    workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
            });
        })
        .detach();
    }
}
