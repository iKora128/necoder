use crate::workspace::*;

impl Workspace {
    /// 編隊 herd サイドバー（状態一覧・M14）の開閉。todo/git と排他（explorer が既定ビュー）。
    /// レールの ⚡ アイコン / コマンドパレット「表示: 編隊」から呼ぶ。
    pub(crate) fn toggle_herd_sidebar(
        &mut self,
        _: &ToggleHerdSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.show_herd {
            self.chrome.show_herd = false;
            // 閉じたらエディタへフォーカスを戻す（git パネルと同じ所作）。
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
        } else {
            self.chrome.show_herd = true;
            self.chrome.show_left = true;
            // 排他: 他の左ドックボード（todo/git）は畳む。
            self.todo_panel
                .update(cx, |panel, cx| panel.set_open(false, cx));
            self.git_panel
                .update(cx, |panel, cx| panel.set_open(false, cx));
            self.ensure_fleet_clock(cx); // 行の相対時刻（開始/入力）を古びさせない
        }
        cx.notify();
    }

    /// Task 見出し / セルヘッダのダブルクリック → 改名（スレッドタブと同じ EditorView・IME 対応・2026-07-24）。
    /// `site` = 入力欄をどこに描くか（herd 見出し / セルヘッダ）。編隊モードで両方が同時に見えても
    /// 同じ入力欄 Entity を二重描画しないための識別。改名は表示名（title）だけを変える。
    pub(crate) fn start_task_rename(
        &mut self,
        project_index: usize,
        site: RenameSite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(project_index) else {
            return;
        };
        let color = slot.color;
        let title = slot.task_space.title.clone();
        let editor = cx.new(|cx| {
            let mut view = EditorView::plain(self.theme.clone(), color, true, cx);
            view.set_plain_text(title.as_ref(), cx);
            view
        });
        cx.subscribe(&editor, |workspace, _editor, event, cx| match event {
            ComposerEvent::Submit => workspace.confirm_task_rename(cx),
        })
        .detach();
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.chrome.task_renaming = Some(TaskRenaming {
            index: project_index,
            site,
            editor,
        });
        cx.notify();
    }

    /// Task 改名の確定（Enter・空白のみは無視）。title と slot 名を揃えて台帳へ永続化。
    fn confirm_task_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.chrome.task_renaming.take() else {
            return;
        };
        let text = renaming.editor.read(cx).plain_text();
        let name = text.trim();
        if !name.is_empty() {
            if let Some(slot) = self.project_sessions.projects.get_mut(renaming.index) {
                slot.task_space.title = SharedString::from(name.to_string());
                slot.name = SharedString::from(name.to_string());
            }
            self.persist_task_space(renaming.index, cx);
        }
        cx.notify();
    }

    /// herd サイドバー本体（M14 #1・herdr の「設定不要の状態一覧」を ACP ネイティブで）。
    /// ウィンドウのレールに載る全プロジェクトのスレッド状態を**プロジェクト別グループ**で一覧する。
    /// 行 = 左 2px スレッド色バー ＋ `activity_dot`（形と動き）＋ エージェントアイコン ＋
    /// 名前 / ⎇ブランチ・状態 ＋ トークン（UI-SPEC §11）。**色相は識別、状態は形と動き**（§1.3）。
    /// クリックでそのプロジェクト＋スレッドへ飛ぶ（focus-follows の入口・M14 #3 で worktree 追従へ拡張）。
    pub(crate) fn render_herd_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use agent_panel::ThreadActivity;
        let theme = self.theme.clone();

        // 収集: プロジェクト別のスレッド状態（各 session の agent_panel から・非アクティブも含む）。
        // agent_panel の read はメモリ参照のみ（Host/FS 呼び出しなし＝描画中に呼んで安全）。
        struct HerdGroup {
            project_index: usize,
            name: SharedString,
            color: Hsla,
            branch: Option<SharedString>,
            statuses: Vec<agent_panel::AgentStatus>,
            is_integration: bool,
        }
        let mut groups: Vec<HerdGroup> = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            let statuses = session.agent_panel.read(cx).statuses();
            if statuses.is_empty() {
                continue;
            }
            // アーカイブ済み Task は一覧から消す（完全削除は見出し右クリックのメニューから・3 層の中段）。
            if !slot.task_space.is_integration() && slot.task_space.phase == TaskPhase::Archived {
                continue;
            }
            groups.push(HerdGroup {
                project_index: index,
                name: slot.name.clone(),
                color: slot.color,
                is_integration: slot.task_space.is_integration(),
                // worktree space の識別 = 現在ブランチ（無ければ開いた時の worktree ブランチ）。
                // ①「worktree = 切替 space」＝この branch がグループ（＝space）の見出しになる。
                branch: slot
                    .branch
                    .clone()
                    .or_else(|| slot.worktree_branch.clone())
                    .map(SharedString::from),
                statuses,
            });
        }

        // パネル見出しは置かない（mock 準拠・`.fleet-left` は群見出しへ直行）。モード名はタイトルバーの
        // 「Multi Agent」トグルが、稼働数のロールアップは下のステータスバー中央が既に担う（重複を避ける）。

        // リスト本体（プロジェクト別グループ）。
        let mut list = div()
            .id("herd-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(4.));
        let mut seq = 0usize;
        // 編隊モードでは solo（Integration）のスレッド群を既定で畳む（Task が主役・見出しクリックで展開）。
        let collapse_solo = self.chrome.fleet_mode && !self.chrome.herd_solo_expanded;
        for group in &groups {
            let collapsed = collapse_solo && group.is_integration;
            // グループ見出しの右肩 = スレッド数（状態は各行と `activity_dot` が見せるので、ここでは
            // 誤解を生む集約語を出さず件数だけ）。全体の実行中/承認待ち内訳はパネルヘッダのロールアップに。
            let mini = group.statuses.len().to_string();
            let solo_toggle = self.chrome.fleet_mode && group.is_integration;
            let is_task_group = !group.is_integration;
            let project_index = group.project_index;
            let renaming_editor = self
                .chrome
                .task_renaming
                .as_ref()
                .filter(|renaming| {
                    renaming.index == project_index && renaming.site == RenameSite::Herd
                })
                .map(|renaming| renaming.editor.clone());
            list = list.child(
                div()
                    .id(("herd-group", group.project_index))
                    .group("herd-head")
                    .when(solo_toggle, |element| {
                        element.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _e: &MouseDownEvent, _window, cx| {
                                this.chrome.herd_solo_expanded = !this.chrome.herd_solo_expanded;
                                cx.notify();
                            }),
                        )
                    })
                    // Task 見出し: ダブルクリック=改名 / 右クリック=メニュー（アーカイブ・worktree/
                    // ブランチ削除の完全削除まで。レールから外れた Task の削除入口・2026-07-24）。
                    .when(is_task_group, |element| {
                        element
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    if event.click_count == 2 {
                                        this.start_task_rename(
                                            project_index,
                                            RenameSite::Herd,
                                            window,
                                            cx,
                                        );
                                    }
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                    this.open_rail_menu(project_index, event.position, cx);
                                }),
                            )
                    })
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .pt(px(8.))
                    .pb(px(3.))
                    .child(
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .bg(group.color)
                            .flex_none(),
                    )
                    .child(match renaming_editor {
                        Some(editor) => div()
                            .flex_1()
                            .min_w_0()
                            .h(px(20.))
                            .child(editor)
                            .into_any_element(),
                        None => div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.fg1)
                            .child(group.name.clone())
                            .into_any_element(),
                    })
                    // ⎇ ブランチ = この space が「どの worktree か」の識別（①の核）。
                    // これが出ることで herd が「プロジェクト一覧」でなく「worktree space 一覧」に読める。
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .when_some(group.branch.clone(), |element, branch| {
                                element.child(SharedString::from(format!("⎇ {branch}")))
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(mini)),
                    )
                    // worktree を削除 🗑（ホバーで出現）。押すと「失うものを数える」確認ダイアログ
                    // （レール右クリック・セルの ⋯ と同じ `request_worktree_delete` へ委譲＝どこから消しても同じ）。
                    .when(is_task_group, |element| {
                        element.child(
                            div()
                                .id(("herd-head-trash", project_index))
                                .group("herd-head-trash")
                                .flex_none()
                                .invisible()
                                .group_hover("herd-head", |style| style.visible())
                                .size(px(16.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .cursor_pointer()
                                .hover(|style| style.bg(theme.bg2))
                                // svg は親の text_color を継承しないため直接指定 + hover は自グループで赤に。
                                .child(
                                    svg()
                                        .path("icons/trash-2.svg")
                                        .size(px(10.))
                                        .text_color(theme.fg2)
                                        .group_hover("herd-head-trash", |style| {
                                            style.text_color(theme.err)
                                        }),
                                )
                                .tooltip(Tooltip::text(
                                    i18n::t!("fleet.delete_worktree_tip"),
                                    theme.clone(),
                                ))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(
                                        move |this, _event: &MouseDownEvent, window, cx| {
                                            cx.stop_propagation();
                                            this.request_worktree_delete(
                                                project_index,
                                                false,
                                                window,
                                                cx,
                                            );
                                        },
                                    ),
                                ),
                        )
                    })
                    .when(solo_toggle, |element| {
                        element.child(
                            div()
                                .flex_none()
                                .text_size(px(9.))
                                .text_color(theme.fg2)
                                .child(if collapsed { "▸" } else { "⌄" }),
                        )
                    }),
            );
            if collapsed {
                continue; // 見出しだけ（件数と ▸ が「居ることは分かる」を担保）
            }
            for (thread_index, status) in group.statuses.iter().enumerate() {
                let color = status.color;
                let activity = status.activity;
                // 行は状態 + 時刻 1 つ（幅が狭いので、入力済みなら最終入力・未入力なら開始だけ。
                // 両方はホバーのツールチップで・M14。⎇ ブランチはグループ見出し＝worktree space が持つ）。
                let latest_time = match status.last_input_at_ms {
                    Some(last_input_at_ms) => i18n::t!(
                        "time.last_input",
                        "when" => agent_panel::relative_time_label(last_input_at_ms)
                    ),
                    None => i18n::t!(
                        "time.started",
                        "when" => agent_panel::relative_time_label(status.created_at_ms)
                    ),
                };
                // 遷移スナップショット（P1）: Blocked=許可待ちの内容 / Done=最後の発言の末尾 /
                // Working=ライブ素材。状態そのものはドット（形と動き）が担うので、文は「中身」を出す。
                // digest が無いスレッド（未実行等）は従来の「状態 · 時刻」。
                let sub = match &status.digest {
                    Some(digest) => digest.clone(),
                    None => {
                        SharedString::from(format!("{} · {latest_time}", activity_label(activity)))
                    }
                };
                let times_tooltip =
                    thread_times_label(status.created_at_ms, status.last_input_at_ms);
                let tokens = if status.tokens_used == 0 {
                    SharedString::from("—")
                } else {
                    SharedString::from(agent_panel::human_tokens(status.tokens_used))
                };
                let project_index = group.project_index;
                list = list.child(
                    div()
                        .id(("herd-row", seq))
                        .group("herd-row")
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_h(px(38.))
                        .pl(px(6.))
                        .pr(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3))
                        // 開始/最終入力の両方はホバーで（行内は幅の都合で最新の 1 つだけ・M14）。
                        .tooltip(Tooltip::text(times_tooltip, theme.clone()))
                        // 左 2px スレッド色バー（帰属＝どのスレッドか・UI-SPEC §11）。
                        .child(
                            div()
                                .w(px(2.5))
                                .h(px(24.))
                                .rounded_full()
                                .bg(color)
                                .flex_none(),
                        )
                        // 状態ドット（形と動き・色相は識別色のまま・§1.3）。
                        .child(agent_panel::activity_dot(
                            ("herd-dot", seq),
                            9.0,
                            color,
                            activity,
                        ))
                        // エージェント種別アイコン（どのエージェントか＝スレッド色とは別軸・§6）。
                        .child(agent_panel::agent_badge(status.agent.as_ref(), 15.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(12.))
                                        .text_color(theme.fg0)
                                        .child(status.name.clone()),
                                )
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(10.))
                                        .text_color(theme.fg2)
                                        .child(sub),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(theme.fg2)
                                .child(tokens),
                        )
                        // エージェント別ミュート 🔕（P2）。muted 中は常時表示・それ以外はホバーで出現。
                        // 効果 = このスレッドのトースト/完了音を抑止（ニュースには載る＝見えるが鳴らない）。
                        .child({
                            let muted = status.muted;
                            div()
                                .id(("herd-mute", seq))
                                .flex_none()
                                .map(|element| {
                                    if muted {
                                        element
                                    } else {
                                        element
                                            .invisible()
                                            .group_hover("herd-row", |style| style.visible())
                                    }
                                })
                                .size(px(17.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .text_size(px(10.))
                                .text_color(theme.fg2)
                                .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                                // フラットアイコン（絵文字は使わない・2026-07-24）。svg は親の
                                // text_color を継承しないため直接指定（GPUI の罠）。
                                .child(
                                    svg()
                                        .path(if muted {
                                            "icons/bell-off.svg"
                                        } else {
                                            "icons/bell.svg"
                                        })
                                        .size(px(11.))
                                        .text_color(theme.fg2),
                                )
                                .tooltip(Tooltip::text(
                                    i18n::t!(if muted { "herd.unmute" } else { "herd.mute" }),
                                    theme.clone(),
                                ))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(
                                        move |this, _event: &MouseDownEvent, _window, cx| {
                                            cx.stop_propagation();
                                            this.project_sessions.sessions[project_index]
                                                .agent_panel
                                                .update(cx, |panel, cx| {
                                                    panel.toggle_thread_mute(thread_index, cx)
                                                });
                                        },
                                    ),
                                )
                        })
                        // 削除 ×（ホバーで出現）。エージェント（スレッド）をアーカイブして一覧から消す
                        // ＝「セルの × は表示から外すだけ」に対し、こちらが本当の削除（⌘⇧T で復元可）。
                        .child(
                            div()
                                .id(("herd-del", seq))
                                .flex_none()
                                .invisible()
                                .group_hover("herd-row", |style| style.visible())
                                .size(px(17.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .text_size(px(13.))
                                .text_color(theme.fg2)
                                .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                                .child("×")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(
                                        move |this, _event: &MouseDownEvent, _window, cx| {
                                            cx.stop_propagation(); // 行本体（reveal/switch）へ伝播させない
                                            this.close_agent(project_index, thread_index, cx);
                                        },
                                    ),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                                if this.chrome.fleet_mode {
                                    // 編隊モード: そのエージェントをグリッドに出す（無ければセル追加）→拡大。
                                    // ＝閉じたセルもここから戻せる（Agent ドックは編隊では非表示なので開かない）。
                                    this.reveal_agent_in_fleet(
                                        project_index,
                                        thread_index,
                                        window,
                                        cx,
                                    );
                                } else {
                                    // 通常: そのプロジェクトへ切替 → スレッド前面 → Agent ドックを開く。
                                    this.switch_project(project_index, window, cx);
                                    let panel = this.project_sessions.sessions[project_index]
                                        .agent_panel
                                        .clone();
                                    panel.update(cx, |panel, cx| {
                                        panel.focus_thread(thread_index, cx)
                                    });
                                    this.chrome.show_right = true;
                                    this.agent_active = true;
                                    cx.notify();
                                }
                            }),
                        ),
                );
                seq += 1;
            }
        }

        let body = if groups.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px(px(16.))
                .text_size(px(11.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("herd.empty")))
                .into_any_element()
        } else {
            list.into_any_element()
        };

        // 凡例（**形の説明**・色は識別に使うので中立色 fg2 で・UI-SPEC §11「末尾に5状態の凡例」）。
        let neutral = theme.fg2;
        let legend_states = [
            ThreadActivity::Working,
            ThreadActivity::Blocked,
            ThreadActivity::Done { interrupted: false },
            ThreadActivity::Idle,
        ];
        let mut legend = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.))
            .border_t_1()
            .border_color(theme.border);
        for (index, activity) in legend_states.into_iter().enumerate() {
            legend = legend.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(agent_panel::activity_dot(
                        ("herd-legend", index),
                        7.0,
                        neutral,
                        activity,
                    ))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(activity_label(activity)),
                    ),
            );
        }

        // 下段: focus 中エージェントの worktree ツリー（mock `.fleet-files`・UI-SPEC §11）。
        // herd は explorer と排他表示なので、herd ビュー中はここが唯一のファイルツリーになる（重複しない）。
        // モデルは「1 パネル=1 プロジェクト」＝ focus 中スレッドの worktree = アクティブ project の
        // worktree。herd 行クリックが `switch_project` するので、ツリーはフォーカスに追従する
        // （左エクスプローラ/エディタ/下ターミナルまで張り替える完全な focus-follows は M14 #3）。
        let files = self.active_slot().map(|slot| {
            let panel = self.agent_panel.read(cx);
            let focus_name = panel
                .active_thread_name()
                .unwrap_or_else(|| slot.name.clone());
            let focus_color = panel.active_color();
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(28.))
                        .px(px(10.))
                        // focus 中スレッドの色ドット（どのエージェントの worktree かの手がかり）。
                        .child(
                            div()
                                .size(px(7.))
                                .rounded_full()
                                .bg(focus_color)
                                .flex_none(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from(
                                    i18n::t!("herd.files_of", "name" => focus_name),
                                )),
                        ),
                )
                .child(self.render_tree(slot, cx))
                .into_any_element()
        });

        div()
            .w(px(self.chrome.explorer_width))
            .h_full()
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .child(body)
            .child(legend)
            .children(files)
            .child(self.left_dock_resize_handle(cx))
            .into_any_element()
    }
}
