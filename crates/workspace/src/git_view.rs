impl Workspace {
    fn render_git_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let (bg0, bg1, bg2, border, fg0, fg1, fg2) =
            (theme.bg0, theme.bg1, theme.bg2, theme.border, theme.fg0, theme.fg1, theme.fg2);
        let Some(state) = self.git_panel.as_ref() else {
            return div().w(px(self.chrome.explorer_width)).h_full().flex_none().bg(bg1).into_any_element();
        };
        let focus = state.focus.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        if self.active_slot().is_none() {
            return div()
                .w(px(self.chrome.explorer_width))
                .h_full()
                .flex_none()
                .bg(bg1)
                .border_r_1()
                .border_color(border)
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_git_key_down))
                .child(div().p(px(12.)).text_size(px(11.5)).text_color(fg2).child(SharedString::from(i18n::t!("git.no_project"))))
                .into_any_element();
        }
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());
        let changes = &self.git_changes;
        let naming = state.branch_name.is_some();

        // ── ヘッダ（タイトル + ブランチ + ＋ + ×）──
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(34.))
            .px(px(10.))
            .flex_none()
            .bg(bg0)
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg1)
                    .child(SharedString::from(i18n::t!("git.title"))),
            )
            .child(div().flex_1())
            .when_some(branch.clone(), |element, branch| {
                element.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .max_w(px(120.))
                        .child(div().flex_none().text_size(px(10.5)).text_color(accent).child("⎇"))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(11.))
                                .text_color(fg2)
                                .child(SharedString::from(branch)),
                        ),
                )
            })
            // GitHub 連携（origin が GitHub のときだけ。PR 作成 / PR・リポジトリを開く）
            .when(self.github_slug.is_some(), |element| {
                element
                    .child(
                        div()
                            .id("git-pr-create")
                            .flex_none()
                            .px(px(5.))
                            .rounded(px(4.))
                            .text_size(px(10.5))
                            .text_color(fg2)
                            .cursor_pointer()
                            .hover(|style| style.bg(bg2).text_color(fg0))
                            .child("PR")
                            .tooltip(Tooltip::text(i18n::t!("git.pr_create_tip"), theme.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| this.github_action(true, window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("git-pr-open")
                            .flex_none()
                            .px(px(5.))
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .text_color(fg2)
                            .cursor_pointer()
                            .hover(|style| style.bg(bg2).text_color(fg0))
                            .child("↗")
                            .tooltip(Tooltip::text(i18n::t!("git.pr_open_tip"), theme.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| this.github_action(false, window, cx)),
                            ),
                    )
            })
            .child(
                div()
                    .id("git-new-branch")
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(bg2).text_color(fg0))
                    .child("＋")
                    .tooltip(Tooltip::text(i18n::t!("git.new_branch_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.start_new_branch(window, cx)),
                    ),
            )
            .child(
                div()
                    .id("git-close")
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(bg2).text_color(fg0))
                    .child("×")
                    .tooltip(Tooltip::text(i18n::t!("git.close_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.toggle_git_panel(&ToggleGitPanel, window, cx)
                        }),
                    ),
            );

        // ── 入力行（コミットメッセージ / ブランチ名）──
        let input_text = match &state.branch_name {
            Some(name) => name.clone(),
            None => state.message.clone(),
        };
        let placeholder = if naming {
            i18n::t!("git.branch_placeholder")
        } else {
            i18n::t!("git.message_placeholder")
        };
        let input_body = if input_text.is_empty() {
            div().text_color(fg2).child(SharedString::from(placeholder))
        } else {
            div().text_color(fg0).child(SharedString::from(format!("{input_text}▍")))
        };
        let input_row = div()
            .id("git-input")
            .m(px(8.))
            .p(px(8.))
            .h(px(46.))
            .bg(bg2)
            .border_1()
            .border_color(if naming { accent } else { border })
            .rounded(px(6.))
            .text_size(px(12.))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if let Some(state) = this.git_panel.as_ref() {
                        let focus = state.focus.clone();
                        window.focus(&focus, cx);
                    }
                }),
            )
            .child(input_body);

        // ── アクション行（commit / push / pull）。ブランチ名モードでは出さない ──
        let commit_ready = !state.message.trim().is_empty();
        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .pb(px(8.))
            .flex_none()
            // ✨ AI でコミットメッセージ生成（Claude Code CLI に diff を渡す）
            .child(
                div()
                    .id("git-ai-message")
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(26.))
                    .h(px(26.))
                    .rounded(px(6.))
                    .bg(bg2)
                    .text_size(px(13.))
                    .text_color(if self.git_busy { fg2 } else { fg1 })
                    .when(!self.git_busy, |element| {
                        element.cursor_pointer().hover(|style| style.bg(theme.bg3).text_color(fg0)).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.generate_commit_message(window, cx)),
                        )
                    })
                    .child("✨")
                    .tooltip(Tooltip::text(i18n::t!("git.ai_message_tip"), theme.clone())),
            )
            .child(
                div()
                    .id("git-commit")
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(26.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .when(commit_ready, |element| {
                        element.bg(accent).text_color(theme.bg0).cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.git_commit(window, cx)),
                        )
                    })
                    .when(!commit_ready, |element| element.bg(bg2).text_color(fg2))
                    .child(SharedString::from(i18n::t!("git.commit"))),
            )
            .child(self.git_remote_button("git-push", "↑", "push", true, cx))
            .child(self.git_remote_button("git-pull", "↓", "pull", false, cx))
            .when(self.git_busy, |element| {
                element.child(div().text_size(px(11.)).text_color(fg2).child("…"))
            });

        // ── 変更一覧（staged / unstaged）。高さを抑えて下に履歴を置く ──
        let mut body = div().flex_none().max_h(px(240.)).flex().flex_col().overflow_hidden().pb(px(6.));
        let staged_count = changes.iter().filter(|change| change.staged.is_some()).count();
        let unstaged_count = changes.iter().filter(|change| change.unstaged.is_some()).count();
        if staged_count > 0 {
            body = body.child(self.git_section_header(&i18n::t!("git.staged"), staged_count, false, cx));
            for (index, change) in changes.iter().filter(|change| change.staged.is_some()).enumerate() {
                if let Some(kind) = change.staged {
                    body = body.child(self.git_change_row(change.path.clone(), kind, true, index, cx));
                }
            }
        }
        if unstaged_count > 0 {
            body = body.child(self.git_section_header(&i18n::t!("git.changes"), unstaged_count, true, cx));
            for (index, change) in changes.iter().filter(|change| change.unstaged.is_some()).enumerate() {
                if let Some(kind) = change.unstaged {
                    body = body.child(self.git_change_row(change.path.clone(), kind, false, index, cx));
                }
            }
        }
        if staged_count == 0 && unstaged_count == 0 {
            body = body.child(
                div().px(px(12.)).py(px(8.)).text_size(px(11.5)).text_color(fg2).child(SharedString::from(i18n::t!("git.no_changes"))),
            );
        }

        div()
            .w(px(self.chrome.explorer_width))
            .h_full()
            .flex()
            .flex_col()
            .flex_none()
            .relative() // リサイズハンドルの絶対配置基準
            .bg(bg1)
            .border_r_1()
            .border_color(border)
            .track_focus(&focus)
            .on_key_down(cx.listener(Self::on_git_key_down))
            .child(header)
            .child(input_row)
            .when(!naming, |element| element.child(actions))
            .child(body)
            .child(div().h(px(1.)).flex_none().bg(border))
            .child(self.render_git_history(&self.git_history))
            .child(self.left_dock_resize_handle(cx))
            .into_any_element()
    }

    /// push/pull の丸ボタン（背景実行中は無効表示）。
    fn git_remote_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: &'static str,
        is_push: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let disabled = self.git_busy;
        div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(26.))
            .h(px(26.))
            .rounded(px(6.))
            .bg(theme.bg2)
            .text_size(px(13.))
            .text_color(if disabled { theme.fg2 } else { theme.fg1 })
            .when(!disabled, |element| {
                element.cursor_pointer().hover(|style| style.bg(theme.bg3).text_color(theme.fg0)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if is_push {
                            this.git_push(window, cx)
                        } else {
                            this.git_pull(window, cx)
                        }
                    }),
                )
            })
            .child(glyph)
            .tooltip(Tooltip::text(label, theme.clone()))
    }

    /// 変更一覧のセクション見出し（"ステージ済み"/"変更" + 件数。"変更"側は「すべてステージ」付き）。
    fn git_section_header(
        &self,
        title: &str,
        count: usize,
        stage_all: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(3.))
            .pt(px(6.))
            .flex_none()
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg2)
                    .child(format!("{title}  {count}")),
            )
            .child(div().flex_1())
            .when(stage_all, |element| {
                element.child(
                    div()
                        .id("git-stage-all")
                        .px(px(5.))
                        .rounded(px(4.))
                        .text_size(px(13.))
                        .text_color(theme.fg2)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                        .child("＋")
                        .tooltip(Tooltip::text(i18n::t!("git.stage_all_tip"), theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.git_stage_all(cx)),
                        ),
                )
            })
    }

    /// 変更 1 行（色付きレター + ファイル名 + stage/unstage ボタン）。
    fn git_change_row(
        &self,
        path: PathBuf,
        kind: StatusKind,
        staged: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let tint = Self::git_tint(&theme, kind);
        let letter = Self::git_letter(kind);
        let name =
            path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let row_id = if staged { "git-staged" } else { "git-unstaged" };
        let act_id = if staged { "git-unstage" } else { "git-stage" };
        let action_path = path.clone();
        div()
            .id((row_id, index))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(3.))
            .text_size(px(12.))
            .hover(|style| style.bg(theme.bg3))
            .child(div().w(px(12.)).flex_none().text_size(px(11.)).text_color(tint).child(letter))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme.fg1)
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .id((act_id, index))
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                    .child(if staged { "−" } else { "＋" })
                    .tooltip(Tooltip::text(
                        SharedString::from(if staged { i18n::t!("git.unstage") } else { i18n::t!("git.stage") }),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            if staged {
                                this.git_unstage(action_path.clone(), cx)
                            } else {
                                this.git_stage(action_path.clone(), cx)
                            }
                        }),
                    ),
            )
            .into_any_element()
    }

    /// git 履歴（コミットグラフ・M8）。レーン線＝矩形（railway）で描き、色はプロジェクト色
    /// パレットに乗せる（ブランチ＝色。「色による方向感覚」）。点/線/コネクタは絶対配置の div。
    fn render_git_history(&self, commits: &[GraphCommit]) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        // 描画幅のための最大レーン。
        let max_lane = commits
            .iter()
            .map(|commit| {
                commit
                    .lanes_in
                    .iter()
                    .chain(&commit.lanes_out)
                    .chain(&commit.connectors)
                    .copied()
                    .fold(commit.dot_lane, usize::max)
            })
            .max()
            .unwrap_or(0);
        let lane_w = 14.0_f32;
        let row_h = 22.0_f32;
        let mid = row_h / 2.0;
        let thick = 1.5_f32;
        let dot_r = 3.0_f32;
        let cell_w = (max_lane as f32 + 1.0) * lane_w;
        let lane_x = |lane: usize| lane as f32 * lane_w + lane_w / 2.0;

        let mut list = div().flex().flex_col().flex_1().min_h_0().overflow_hidden().child(
            div()
                .flex_none()
                .px(px(10.))
                .py(px(3.))
                .pt(px(6.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("git.history"))),
        );

        for commit in commits {
            // ── グラフセル（点 + 縦レーン + 横コネクタ）──
            let mut cell = div().relative().flex_none().h(px(row_h)).w(px(cell_w));
            for &lane in &commit.lanes_in {
                cell = cell.child(
                    div()
                        .absolute()
                        .left(px(lane_x(lane) - thick / 2.0))
                        .top(px(0.))
                        .w(px(thick))
                        .h(px(mid))
                        .bg(project_color(lane)),
                );
            }
            for &lane in &commit.lanes_out {
                cell = cell.child(
                    div()
                        .absolute()
                        .left(px(lane_x(lane) - thick / 2.0))
                        .top(px(mid))
                        .w(px(thick))
                        .h(px(row_h - mid))
                        .bg(project_color(lane)),
                );
            }
            for &lane in &commit.connectors {
                let start = lane_x(commit.dot_lane).min(lane_x(lane));
                let end = lane_x(commit.dot_lane).max(lane_x(lane));
                cell = cell.child(
                    div()
                        .absolute()
                        .left(px(start))
                        .top(px(mid - thick / 2.0))
                        .w(px(end - start))
                        .h(px(thick))
                        .bg(project_color(commit.dot_lane)),
                );
            }
            cell = cell.child(
                div()
                    .absolute()
                    .left(px(lane_x(commit.dot_lane) - dot_r))
                    .top(px(mid - dot_r))
                    .w(px(dot_r * 2.0))
                    .h(px(dot_r * 2.0))
                    .rounded(px(dot_r))
                    .bg(project_color(commit.dot_lane)),
            );

            // ── テキスト（ref チップ + 要約 + hash）──
            let mut text =
                div().flex_1().flex().items_center().gap(px(6.)).overflow_hidden().whitespace_nowrap();
            for reference in &commit.refs {
                let is_head = reference.contains("HEAD");
                let label = reference.trim_start_matches("HEAD -> ").to_string();
                text = text.child(
                    div()
                        .flex_none()
                        .px(px(4.))
                        .rounded(px(3.))
                        .text_size(px(9.5))
                        .bg(theme.bg2)
                        .text_color(if is_head { accent } else { theme.fg2 })
                        .child(SharedString::from(label)),
                );
            }
            text = text
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .child(SharedString::from(commit.summary.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(commit.short_hash.clone())),
                );

            list = list.child(
                div().flex().items_center().gap(px(6.)).px(px(10.)).h(px(row_h)).child(cell).child(text),
            );
        }
        if commits.is_empty() {
            list = list.child(
                div().px(px(12.)).py(px(6.)).text_size(px(11.)).text_color(theme.fg2).child(SharedString::from(i18n::t!("git.no_commits"))),
            );
        }
        list
    }

    fn render_branch_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.branch_menu.as_ref()?;
        let position = menu.position;
        let slot = self.active_slot()?;
        let theme = self.theme.clone();
        let accent = slot.color;
        let current = menu.current.clone();
        let branches = menu.branches.clone();
        let worktrees = menu.worktrees.clone();

        let (bg2, bg3, border, fg0, fg1, fg2) =
            (theme.bg2, theme.bg3, theme.border, theme.fg0, theme.fg1, theme.fg2);

        let mut menu_box = div()
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(280.))
            .bg(bg2)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
            ])
            .child(div().px(px(8.)).py(px(4.)).text_size(px(10.5)).text_color(fg2).child(SharedString::from(i18n::t!("git.branches"))));

        for (index, branch) in branches.into_iter().enumerate() {
            let is_current = current.as_deref() == Some(branch.as_str());
            let switch_branch = branch.clone();
            let worktree_branch = branch.clone();
            let delete_branch_name = branch.clone();
            menu_box = menu_box.child(
                div()
                    .id(("branch", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|style| style.bg(bg3))
                    .child(
                        div()
                            .w(px(10.))
                            .flex_none()
                            .text_color(accent)
                            .child(if is_current { "●" } else { "" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(if is_current { fg0 } else { fg1 })
                            .child(SharedString::from(branch.clone())),
                    )
                    // 行クリック = in-place 切替（現在ブランチは無効）
                    .when(!is_current, |element| {
                        element.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.switch_branch_to(switch_branch.clone(), window, cx)
                            }),
                        )
                    })
                    // ⧉ = worktree として新しい窓で開く（当初ビジョン）
                    .child(
                        div()
                            .id(("branch-wt", index))
                            .flex_none()
                            .px(px(4.))
                            .rounded(px(4.))
                            .text_size(px(11.))
                            .text_color(fg2)
                            .hover(|style| style.bg(bg2).text_color(fg0))
                            .child("⧉")
                            .tooltip(Tooltip::text(i18n::t!("git.worktree_open_tip"), theme.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.open_branch_worktree(worktree_branch.clone(), cx)
                                }),
                            ),
                    )
                    // 🗑 = ブランチ削除（現在ブランチ以外。未マージは git が -d で拒否＝安全側）
                    .when(!is_current, |element| {
                        let delete_branch_name = delete_branch_name.clone();
                        element.child(
                            div()
                                .id(("branch-del", index))
                                .flex_none()
                                .px(px(4.))
                                .rounded(px(4.))
                                .text_size(px(11.))
                                .text_color(fg2)
                                .hover(|style| style.bg(bg2).text_color(theme.err))
                                .child("🗑")
                                .tooltip(Tooltip::text(i18n::t!("git.delete_branch_tip"), theme.clone()))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.delete_git_branch(delete_branch_name.clone(), cx)
                                    }),
                                ),
                        )
                    }),
            );
        }

        // worktree セクション（現在の作業ツリー以外）。
        let root = slot.worktree.root();
        let others: Vec<_> = worktrees.into_iter().filter(|worktree| worktree.path != root).collect();
        if !others.is_empty() {
            menu_box = menu_box
                .child(div().h(px(1.)).bg(border).my(px(3.)))
                .child(div().px(px(8.)).py(px(4.)).text_size(px(10.5)).text_color(fg2).child("worktree"));
            for (index, worktree) in others.into_iter().enumerate() {
                let label = worktree.branch.clone().unwrap_or_else(|| {
                    worktree
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default()
                });
                let path = worktree.path.clone();
                let branch = worktree.branch.clone();
                menu_box = menu_box.child(
                    div()
                        .id(("worktree", index))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(5.))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|style| style.bg(bg3))
                        .child(div().flex_none().text_color(fg2).child("⎇"))
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(fg1)
                                .child(SharedString::from(label)),
                        )
                        .child(div().flex_none().text_size(px(10.)).text_color(fg2).child(SharedString::from(i18n::t!("git.window_chip"))))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.open_worktree_window(path.clone(), branch.clone(), cx)
                            }),
                        ),
                );
            }
        }

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.hide_branch_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _window, cx| this.hide_branch_menu(cx)),
                )
                .child(menu_box)
                .into_any_element(),
        )
    }

    // アクティブプロジェクト色（無ければパレット先頭）。titlebar ピル左縁・タブ上線に流す。
}
