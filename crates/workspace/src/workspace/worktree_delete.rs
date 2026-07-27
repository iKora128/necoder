//! worktree 削除の確認ダイアログ（2026-07-27・ユーザー要望）。
//!
//! **設計の芯**: 「本当に消しますか？」は情報がゼロなので出す価値が薄い。出すべきなのは
//! **何を失うか**（未コミットの変更 / どこにも残らないコミット）で、それは git に聞けば分かる。
//! なので確認は「実際の損失を数えて見せる」形にし、二段クリック（もう一度で削除）は廃止した。
//!
//! 「次回から確認しない」は **取り返しがつく削除にだけ**効く。未コミットの変更は git にも残らず
//! reflog でも戻せないので、そこを 1 個のチェックボックスで無音化できるようにはしない
//! （設定 `confirm_worktree_delete` の doc と DECISIONS 参照）。

use crate::workspace::*;

/// 確認ダイアログの状態。`stakes` が `None` の間は「調べています…」を出す（git 呼び出しは背景）。
pub(crate) struct WorktreeDeleteConfirm {
    /// 対象のレール index。
    pub(crate) index: usize,
    /// ブランチごと消すか（メニューのどちらから来たか）。
    pub(crate) also_branch: bool,
    pub(crate) name: SharedString,
    pub(crate) path: SharedString,
    pub(crate) branch: Option<SharedString>,
    /// git に聞いた「失うもの」。`None` = 集計中。
    pub(crate) stakes: Option<WorktreeStakes>,
    /// 「次回から確認しない」のチェック状態（確定＝削除ボタンを押した時に settings へ書く）。
    pub(crate) skip_next: bool,
}

/// 消したら失うもの。0/0 なら **同じブランチから作り直せる**＝取り返しがつく。
#[derive(Clone, Copy, Default)]
pub(crate) struct WorktreeStakes {
    /// 未コミットの変更ファイル数（**git にも残らない** = 最も重い損失）。
    pub(crate) dirty_files: usize,
    /// 統合先に取り込まれていないコミット数（`None` = 比較先が無く数えられなかった）。
    pub(crate) unmerged_commits: Option<usize>,
}

impl WorktreeStakes {
    /// 何も失わない（clean かつ未統合コミット 0）。この時だけ「次回から確認しない」が効く。
    pub(crate) fn is_recoverable(self) -> bool {
        self.dirty_files == 0 && self.unmerged_commits == Some(0)
    }
}

impl Workspace {
    /// 削除メニューの入口。**まず失うものを数え**、その結果でダイアログを出すか即実行かを決める。
    /// `confirm_worktree_delete = false` でも、失うものがあれば必ず確認する。
    pub(crate) fn request_worktree_delete(
        &mut self,
        index: usize,
        also_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.rail_menu = None;
        self.chrome.fleet_cell_menu = None;
        let Some(slot) = self.project_sessions.projects.get(index) else {
            return;
        };
        // 最後の 1 枚は削除自体を断る（従来どおり）。ダイアログを出す前にここで弾く。
        if self.project_sessions.projects.len() <= 1 {
            let accent = self.accent();
            self.push_toast(SharedString::from(i18n::t!("rail.cannot_remove_last")), accent, cx);
            return;
        }
        let host = slot.worktree.host().clone();
        let root = slot.worktree.root().to_path_buf();
        let branch = slot
            .branch
            .clone()
            .or_else(|| slot.worktree_branch.clone())
            .filter(|branch| !branch.is_empty());
        self.overlays.worktree_delete = Some(WorktreeDeleteConfirm {
            index,
            also_branch,
            name: slot.task_space.title.clone(),
            path: SharedString::from(root.display().to_string()),
            branch: branch.clone().map(SharedString::from),
            stakes: None,
            skip_next: false,
        });
        cx.notify();

        // 比較先 = 同じリポジトリの IntegrationSpace のブランチ（無ければ main / master を試す）。
        let base_candidates: Vec<String> = self
            .project_sessions
            .projects
            .iter()
            .find(|other| {
                other.task_space.repository_id == slot.task_space.repository_id
                    && other.task_space.is_integration()
            })
            .and_then(|integration| integration.branch.clone())
            .into_iter()
            .chain(["main".to_string(), "master".to_string()])
            .collect();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            let stakes = cx
                .background_executor()
                .spawn(async move {
                    let dirty_files = project::git_status_on(host.as_ref(), &root).len();
                    // 先に解決できた base で数える（消えたブランチ名を持っていることがある）。
                    let unmerged_commits = base_candidates.iter().find_map(|base| {
                        project::git_unmerged_count_on(host.as_ref(), &root, base)
                    });
                    WorktreeStakes { dirty_files, unmerged_commits }
                })
                .await;
            handle
                .update(cx, |workspace, window, cx| {
                    let Some(confirm) = workspace.overlays.worktree_delete.as_mut() else {
                        return; // 集計中に閉じられた
                    };
                    if confirm.index != index {
                        return; // 別の対象に切り替わった
                    }
                    confirm.stakes = Some(stakes);
                    // 取り返しがつく削除で、かつ確認を切ってあるなら黙って実行する。
                    if stakes.is_recoverable() && !settings::get(cx).confirm_worktree_delete {
                        let also_branch = confirm.also_branch;
                        workspace.overlays.worktree_delete = None;
                        workspace.delete_slot_worktree_impl(index, also_branch, window, cx);
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    pub(crate) fn dismiss_worktree_delete(&mut self, cx: &mut Context<Self>) {
        if self.overlays.worktree_delete.take().is_some() {
            cx.notify();
        }
    }

    /// ダイアログの「削除する」。チェックが入っていれば設定へ書いてから実行する。
    fn confirm_worktree_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.overlays.worktree_delete.take() else {
            return;
        };
        if confirm.skip_next {
            settings::set_user_value(cx, "confirm_worktree_delete", serde_json::Value::Bool(false));
        }
        self.delete_slot_worktree_impl(confirm.index, confirm.also_branch, window, cx);
    }

    fn toggle_worktree_delete_skip(&mut self, cx: &mut Context<Self>) {
        if let Some(confirm) = self.overlays.worktree_delete.as_mut() {
            confirm.skip_next = !confirm.skip_next;
            cx.notify();
        }
    }

    /// 中央モーダル。**失うものを数えて見せる**のが主役で、ボタンは添え物。
    pub(crate) fn render_worktree_delete_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let confirm = self.overlays.worktree_delete.as_ref()?;
        let theme = self.theme.clone();
        let stakes = confirm.stakes;
        let recoverable = stakes.is_some_and(WorktreeStakes::is_recoverable);

        // 「失うもの」の行。数が 0 のものは出さない（0 件を並べても読ませるだけ）。
        let mut losses = div().flex().flex_col().gap(px(4.));
        match stakes {
            None => {
                losses = losses.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("worktree_delete.counting"))),
                );
            }
            Some(stakes) => {
                let loss_row = |icon: &'static str, text: SharedString, danger: bool, theme: &Theme| {
                    div()
                        .flex()
                        .items_start()
                        .gap(px(7.))
                        .text_size(px(11.5))
                        .text_color(if danger { theme.err } else { theme.fg1 })
                        .child(div().flex_none().w(px(12.)).child(icon))
                        .child(div().flex_1().min_w_0().child(text))
                };
                if stakes.dirty_files > 0 {
                    losses = losses.child(loss_row(
                        "⚠",
                        SharedString::from(i18n::t!(
                            "worktree_delete.dirty",
                            "n" => stakes.dirty_files
                        )),
                        true,
                        &theme,
                    ));
                }
                match stakes.unmerged_commits {
                    Some(count) if count > 0 => {
                        losses = losses.child(loss_row(
                            "⚠",
                            SharedString::from(i18n::t!(
                                "worktree_delete.unmerged",
                                "n" => count
                            )),
                            true,
                            &theme,
                        ));
                    }
                    None => {
                        losses = losses.child(loss_row(
                            "?",
                            SharedString::from(i18n::t!("worktree_delete.unknown_base")),
                            false,
                            &theme,
                        ));
                    }
                    _ => {}
                }
                if confirm.also_branch {
                    losses = losses.child(loss_row(
                        "⚠",
                        SharedString::from(i18n::t!(
                            "worktree_delete.branch_too",
                            "branch" => confirm.branch.clone().unwrap_or_default()
                        )),
                        true,
                        &theme,
                    ));
                }
                if recoverable && !confirm.also_branch {
                    losses = losses.child(loss_row(
                        "✓",
                        SharedString::from(i18n::t!("worktree_delete.safe")),
                        false,
                        &theme,
                    ));
                }
            }
        }

        let button = |id: &'static str, label: SharedString, danger: bool, theme: &Theme| {
            let (border, text) = if danger {
                (theme.err.alpha(0.7), theme.err)
            } else {
                (theme.border, theme.fg1)
            };
            div()
                .id(id)
                .px(px(14.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(border)
                .text_size(px(12.))
                .text_color(text)
                .cursor_pointer()
                .hover(move |style| style.bg(theme.bg3))
                .child(label)
        };

        // 「次回から確認しない」は取り返しがつく削除にだけ効く ＝ ラベルでそう言い切る。
        let skip_row = div()
            .id("worktree-delete-skip")
            .flex()
            .items_center()
            .gap(px(7.))
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(theme.fg2)
            .hover(|style| style.text_color(theme.fg1))
            .child(
                div()
                    .flex_none()
                    .size(px(13.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if confirm.skip_next { self.accent() } else { theme.border })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.))
                    .text_color(self.accent())
                    .when(confirm.skip_next, |element| element.child("✓")),
            )
            .child(SharedString::from(i18n::t!("worktree_delete.skip_next")))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.toggle_worktree_delete_skip(cx)),
            );

        let card = div()
            .w(px(420.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(10.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .shadow(vec![gpui::BoxShadow::new(px(0.), px(12.), gpui::hsla(0., 0., 0., 0.5))
                .blur_radius(px(28.))])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(if confirm.also_branch {
                        i18n::t!("worktree_delete.title_branch")
                    } else {
                        i18n::t!("worktree_delete.title")
                    })),
            )
            // 対象（名前 ⎇ ブランチ / パス）。**どれを消すのか**を取り違えないための行。
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(7.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.fg0)
                                    .child(confirm.name.clone()),
                            )
                            .when_some(confirm.branch.clone(), |element, branch| {
                                element.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(theme.fg2)
                                        .child(SharedString::from(format!("⎇ {branch}"))),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(confirm.path.clone()),
                    ),
            )
            .child(div().h(px(1.)).bg(theme.border))
            .child(losses)
            .child(skip_row)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("worktree-delete-cancel", SharedString::from(i18n::t!("worktree_delete.cancel")), false, &theme)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _window, cx| this.dismiss_worktree_delete(cx)),
                            ),
                    )
                    .child(
                        button("worktree-delete-go", SharedString::from(i18n::t!("worktree_delete.confirm")), true, &theme)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.confirm_worktree_delete(window, cx)
                                }),
                            ),
                    ),
            );

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.45))
                // 背景クリックでキャンセル（破壊操作なので「外すと実行」には絶対にしない）。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.dismiss_worktree_delete(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}
