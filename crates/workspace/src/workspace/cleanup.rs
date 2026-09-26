//! 片付け（O22・A16）: いまのリポジトリの Task をまとめて片付ける画面。
//!
//! 1 本ずつなら Task カードの ⋯（片付けメニュー）で足りるが、並走させた後は「統合済み・捨てた・
//! 放ってある」Task が溜まる。ここでは Task を一覧し、**消したら失うもの**（未コミットの変更・統合先に
//! 無いコミット）を背景で数えて並べ、選んだものをまとめて:
//!
//! - **終了**（台帳を archived にする・worktree は残す）: 失うものは無いので、どれでも選べる。
//! - **worktree を削除**（ブランチは残す）: **失うものが無い Task だけ**。消す直前にもう一度数え、
//!   その間に変わっていたら消さずに残す（1 本ずつの削除の確認と同じ芯＝失うものを数える）。
//!   失うものがある Task は ⋯ メニューの削除（損失を見せる確認つき）で 1 本ずつ。
//!
//! 削除は 1 本ずつ順に行う（同じリポジトリへ `git worktree remove` を並べると lock でぶつかりうる）。
//! 数えるのは開いた時と ↻ の時だけ。失うものを全部数えてから、各 worktree の大きさ（ディスク上）を
//! 測る（大きいフォルダは時間がかかるので後回し）。「大きい順」で並べ替えられる。

use super::worktree_delete::WorktreeStakes;
use crate::workspace::*;
use std::collections::HashSet;

/// 一覧の 1 行（開いた時点の Task）。
pub(crate) struct CleanupRow {
    space: SpaceId,
    title: SharedString,
    color: Hsla,
    phase: TaskPhase,
    created_at_ms: i64,
    /// `None` = 数えている途中。
    stakes: Option<WorktreeStakes>,
    /// worktree のディスク上の大きさ（バイト）。`None` = 測っている途中・測れない。
    size: Option<u64>,
}

/// 片付けの画面の状態（開いている間だけ Some）。
pub(crate) struct CleanupState {
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    rows: Vec<CleanupRow>,
    selected: HashSet<SpaceId>,
    /// 削除を流している最中（ボタンを止める）。
    busy: bool,
    /// 大きい順に並べる（既定は作った順）。
    by_size: bool,
}

/// 大きさの短い言い方（「1.2 GB」「340 MB」「12 KB」）。
fn size_label(size: Option<u64>) -> String {
    let Some(bytes) = size else {
        return "—".to_string();
    };
    let kib = bytes as f64 / 1024.0;
    if kib < 1024.0 {
        format!("{} KB", kib.round() as u64)
    } else if kib < 1024.0 * 1024.0 {
        format!("{} MB", (kib / 1024.0).round() as u64)
    } else {
        format!("{:.1} GB", kib / (1024.0 * 1024.0))
    }
}

/// 描く順（行の添字）。大きい順なら大きさの降順（測れていない行は後ろ）、でなければ作った順のまま。
fn cleanup_order(rows: &[CleanupRow], by_size: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    if by_size {
        order.sort_by_key(|&index| std::cmp::Reverse(rows[index].size.unwrap_or(0)));
    }
    order
}

/// 状態の短い呼び名（カードの状態チップと同じ語）。
fn phase_label(phase: TaskPhase) -> String {
    let key = match phase {
        TaskPhase::Blocked => "fleet.phase_blocked",
        TaskPhase::Failed | TaskPhase::ChangesRequested => "fleet.phase_failed",
        TaskPhase::ReviewReady | TaskPhase::MergeReady => "fleet.phase_review",
        TaskPhase::Integrating | TaskPhase::Integrated => "fleet.phase_integrated",
        TaskPhase::Archived => "cleanup.phase_archived",
        TaskPhase::Planned | TaskPhase::Working => "fleet.phase_working",
    };
    i18n::t!(key)
}

/// 失うものの短い言い方（行の右）。
fn stakes_label(stakes: Option<WorktreeStakes>) -> (String, bool) {
    match stakes {
        None => (i18n::t!("cleanup.counting"), false),
        Some(stakes) if stakes.is_recoverable() => (i18n::t!("cleanup.nothing_to_lose"), false),
        Some(stakes) => {
            let mut parts = Vec::new();
            if stakes.dirty_files > 0 {
                parts.push(i18n::t!("cleanup.dirty", "n" => stakes.dirty_files));
            }
            match stakes.unmerged_commits {
                Some(count) if count > 0 => {
                    parts.push(i18n::t!("cleanup.unmerged", "n" => count));
                }
                None => parts.push(i18n::t!("cleanup.unknown_base")),
                Some(_) => {}
            }
            (parts.join(" · "), true)
        }
    }
}

impl Workspace {
    /// 比較先の候補（統合先のブランチ → main → master）。1 本ずつの削除の確認と同じ決め方。
    fn stakes_bases_for(&self, session_index: usize) -> Vec<String> {
        let Some(slot) = self.project_sessions.projects.get(session_index) else {
            return Vec::new();
        };
        self.project_sessions
            .projects
            .iter()
            .find(|other| {
                other.task_space.repository_id == slot.task_space.repository_id
                    && other.task_space.is_integration()
            })
            .and_then(|integration| integration.branch.clone())
            .into_iter()
            .chain(["main".to_string(), "master".to_string()])
            .collect()
    }

    /// パレット「Task: 片付け…」。いまのリポジトリの Task を並べ、失うものを背景で数える。
    pub(crate) fn show_cleanup(
        &mut self,
        _: &ShowCleanup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.active_repository_key().map(str::to_string) else {
            return;
        };
        let rows: Vec<CleanupRow> = self
            .project_sessions
            .projects
            .iter()
            .filter(|slot| slot.repository_key() == key && !slot.task_space.is_integration())
            .map(|slot| CleanupRow {
                space: slot.task_space.id.clone(),
                title: slot.task_space.title.clone(),
                color: slot.color,
                phase: slot.task_space.phase,
                created_at_ms: slot.task_space.created_at_ms,
                stakes: None,
                size: None,
            })
            .collect();
        let previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.overlays.cleanup = Some(CleanupState {
            focus,
            previous_focus,
            rows,
            selected: HashSet::new(),
            busy: false,
            by_size: false,
        });
        self.count_cleanup_stakes(cx);
        cx.notify();
    }

    /// 開いている片付けの画面で、この Task たちに印を付ける（Fleet サイドバーの複数選択から・O21）。
    /// 画面に無い Task（別のリポジトリ・統合先）は数えない。
    pub(crate) fn select_cleanup_rows(&mut self, spaces: &[SpaceId]) {
        if let Some(state) = self.overlays.cleanup.as_mut() {
            state.selected = state
                .rows
                .iter()
                .filter(|row| spaces.contains(&row.space))
                .map(|row| row.space.clone())
                .collect();
        }
    }

    /// テスト用: 片付けの画面で印を付けている Task（閉じていれば None）。
    #[cfg(test)]
    pub(crate) fn cleanup_selection(&self) -> Option<HashSet<SpaceId>> {
        self.overlays
            .cleanup
            .as_ref()
            .map(|state| state.selected.clone())
    }

    /// 各 Task の失うものを背景で数える（1 本ずつ・数えた順に反映）。
    fn count_cleanup_stakes(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.cleanup.as_mut() else {
            return;
        };
        for row in &mut state.rows {
            row.stakes = None;
            row.size = None;
        }
        let spaces: Vec<SpaceId> = state.rows.iter().map(|row| row.space.clone()).collect();
        let jobs: Vec<(SpaceId, Arc<dyn host::Host>, PathBuf, Vec<String>)> = spaces
            .into_iter()
            .filter_map(|space| {
                let index = self.session_index_for_space(&space)?;
                let slot = &self.project_sessions.projects[index];
                Some((
                    space,
                    slot.worktree.host().clone(),
                    slot.worktree.root().to_path_buf(),
                    self.stakes_bases_for(index),
                ))
            })
            .collect();
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let sizes: Vec<(SpaceId, Arc<dyn host::Host>, PathBuf)> = jobs
                .iter()
                .map(|(space, host, root, _)| (space.clone(), host.clone(), root.clone()))
                .collect();
            for (space, host, root, bases) in jobs {
                let stakes = cx
                    .background_executor()
                    .spawn(async move { measure_stakes(host.as_ref(), &root, &bases) })
                    .await;
                let applied = workspace.update(cx, |workspace, cx| {
                    let Some(state) = workspace.overlays.cleanup.as_mut() else {
                        return false; // 閉じられた
                    };
                    if let Some(row) = state.rows.iter_mut().find(|row| row.space == space) {
                        row.stakes = Some(stakes);
                    }
                    cx.notify();
                    true
                });
                if !matches!(applied, Ok(true)) {
                    return;
                }
            }
            // 大きさは失うものを全部数えてから（node_modules 等で時間がかかる）。
            for (space, host, root) in sizes {
                let size = cx
                    .background_executor()
                    .spawn(async move { project::disk_usage_on(host.as_ref(), &root) })
                    .await;
                let applied = workspace.update(cx, |workspace, cx| {
                    let Some(state) = workspace.overlays.cleanup.as_mut() else {
                        return false; // 閉じられた
                    };
                    if let Some(row) = state.rows.iter_mut().find(|row| row.space == space) {
                        row.size = size;
                    }
                    cx.notify();
                    true
                });
                if !matches!(applied, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    /// 並べ順を切り替える（作った順 ⇄ 大きい順）。
    fn toggle_cleanup_order(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.overlays.cleanup.as_mut() {
            state.by_size = !state.by_size;
            cx.notify();
        }
    }

    fn close_cleanup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.cleanup.take() else {
            return;
        };
        if let Some(previous) = state.previous_focus {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn toggle_cleanup_row(&mut self, space: SpaceId, cx: &mut Context<Self>) {
        if let Some(state) = self.overlays.cleanup.as_mut() {
            if !state.selected.remove(&space) {
                state.selected.insert(space);
            }
            cx.notify();
        }
    }

    /// 統合済み・終了済みをまとめて選ぶ（いちばんよく片付ける物）。
    fn select_finished_cleanup_rows(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.overlays.cleanup.as_mut() {
            state.selected = state
                .rows
                .iter()
                .filter(|row| matches!(row.phase, TaskPhase::Integrated | TaskPhase::Archived))
                .map(|row| row.space.clone())
                .collect();
            cx.notify();
        }
    }

    /// 選んだ Task を終了（台帳を archived・worktree は残す）。
    fn archive_selected_cleanup_rows(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.cleanup.as_mut() else {
            return;
        };
        let selected: Vec<SpaceId> = state.selected.drain().collect();
        let mut archived = 0;
        for space in selected {
            if let Some(index) = self.session_index_for_space(&space) {
                self.transition_task_space(index, TaskPhase::Archived, "task_cleaned_up", None, cx);
                archived += 1;
                if let Some(row) = self
                    .overlays
                    .cleanup
                    .as_mut()
                    .and_then(|state| state.rows.iter_mut().find(|row| row.space == space))
                {
                    row.phase = TaskPhase::Archived;
                }
            }
        }
        let accent = self.accent();
        self.push_toast(
            i18n::t!("cleanup.archived", "count" => archived).into(),
            accent,
            cx,
        );
    }

    /// 選んだ Task のうち**失うものが無い**物の worktree を消す（ブランチは残す）。1 本ずつ・消す直前に
    /// 数え直し、変わっていたら残す。消えた Task はレールと舞台から外す。
    fn delete_selected_cleanup_worktrees(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(state) = self.overlays.cleanup.as_mut() else {
            return;
        };
        if state.busy {
            return;
        }
        let recoverable: Vec<SpaceId> = state
            .rows
            .iter()
            .filter(|row| {
                state.selected.contains(&row.space)
                    && row.stakes.is_some_and(WorktreeStakes::is_recoverable)
            })
            .map(|row| row.space.clone())
            .collect();
        if recoverable.is_empty() {
            return;
        }
        state.busy = true;
        let mut jobs = Vec::new();
        for space in &recoverable {
            let Some(index) = self.session_index_for_space(space) else {
                continue;
            };
            // 消す前にそこで走っているエージェントを止める（削除中に worktree へ書かれるのを防ぐ）。
            for panel in &self.project_sessions.sessions[index].fleet_agents {
                panel.update(cx, |panel, cx| panel.cancel_all_turns(cx));
            }
            let slot = &self.project_sessions.projects[index];
            jobs.push((
                space.clone(),
                slot.worktree.host().clone(),
                slot.worktree.root().to_path_buf(),
                self.stakes_bases_for(index),
            ));
        }
        cx.notify();
        cx.spawn(async move |_workspace, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    jobs.into_iter()
                        .map(|(space, host, root, bases)| {
                            let outcome = delete_if_nothing_to_lose(host.as_ref(), &root, &bases);
                            (space, root, outcome)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            // Err = 消している間に窓が閉じた（消せた worktree は次に開いた時に「消えています」に出る）。
            handle
                .update(cx, move |workspace, window, cx| {
                    let mut deleted = 0;
                    let mut kept = 0;
                    for (space, root, outcome) in results {
                        match outcome {
                            Ok(true) => {
                                deleted += 1;
                                if let Some(index) = workspace
                                    .project_sessions
                                    .projects
                                    .iter()
                                    .position(|slot| slot.worktree.root() == root.as_path())
                                {
                                    workspace.remove_fleet_cells_for(&space);
                                    workspace.remove_project_slot(index, window, cx);
                                }
                                if let Some(state) = workspace.overlays.cleanup.as_mut() {
                                    state.rows.retain(|row| row.space != space);
                                    state.selected.remove(&space);
                                }
                            }
                            Ok(false) => kept += 1,
                            Err(error) => {
                                kept += 1;
                                eprintln!("worktree を消せない: {error:#}");
                            }
                        }
                    }
                    if let Some(state) = workspace.overlays.cleanup.as_mut() {
                        state.busy = false;
                    }
                    let accent = workspace.accent();
                    workspace.push_toast(
                        i18n::t!("cleanup.deleted", "count" => deleted, "kept" => kept).into(),
                        accent,
                        cx,
                    );
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    /// 中央のモーダル（幅 620）。上に選ぶ操作、Task の行（選ぶ・状態・いつ・失うもの）、下に実行ボタン。
    pub(crate) fn render_cleanup(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.cleanup.as_ref()?;
        let theme = self.theme.clone();
        let selected_count = state.selected.len();
        let deletable = state
            .rows
            .iter()
            .filter(|row| {
                state.selected.contains(&row.space)
                    && row.stakes.is_some_and(WorktreeStakes::is_recoverable)
            })
            .count();
        let button = |id: &'static str, label: String, enabled: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(9.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.))
                .when(enabled, |button| {
                    button
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                })
                .when(!enabled, |button| button.text_color(theme.fg2))
                .child(SharedString::from(label))
        };
        let mut list = div().flex().flex_col().gap(px(2.));
        if state.rows.is_empty() {
            list = list.child(
                div()
                    .py(px(8.))
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("cleanup.empty"))),
            );
        }
        for (position, row) in cleanup_order(&state.rows, state.by_size)
            .into_iter()
            .map(|index| &state.rows[index])
            .enumerate()
        {
            let selected = state.selected.contains(&row.space);
            let (losses, danger) = stakes_label(row.stakes);
            let space = row.space.clone();
            list = list.child(
                div()
                    .id(("cleanup-row", position))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_h(px(30.))
                    .px(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    .child(
                        div()
                            .flex_none()
                            .size(px(13.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme.fg2)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(9.))
                            .text_color(theme.fg0)
                            .when(selected, |check| check.child("✓")),
                    )
                    .child(div().flex_none().size(px(8.)).rounded(px(2.)).bg(row.color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .text_color(theme.fg0)
                            .child(row.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(76.))
                            .text_size(px(10.5))
                            .text_color(theme.fg1)
                            .child(SharedString::from(phase_label(row.phase))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(64.))
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(agent_panel::relative_time_label(row.created_at_ms)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(150.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(10.5))
                            .text_color(if danger { theme.warn } else { theme.fg2 })
                            .child(SharedString::from(losses)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(56.))
                            .flex()
                            .justify_end()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(size_label(row.size))),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_cleanup_row(space.clone(), cx);
                        }),
                    ),
            );
        }
        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!(
                        "cleanup.selected",
                        "count" => selected_count,
                        "deletable" => deletable
                    ))),
            )
            .child(
                button(
                    "cleanup-archive",
                    i18n::t!("cleanup.archive"),
                    selected_count > 0,
                )
                .when(selected_count > 0, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.archive_selected_cleanup_rows(cx);
                        }),
                    )
                }),
            )
            .child(
                button(
                    "cleanup-delete",
                    i18n::t!("cleanup.delete", "count" => deletable),
                    deletable > 0 && !state.busy,
                )
                .when(deletable > 0 && !state.busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.delete_selected_cleanup_worktrees(window, cx);
                        }),
                    )
                }),
            );
        let card = div()
            .w(px(680.))
            .max_h(px(520.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(16.))
            .rounded(px(10.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .track_focus(&state.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_cleanup(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(i18n::t!("cleanup.title"))),
                    )
                    .child(
                        button(
                            "cleanup-order",
                            if state.by_size {
                                i18n::t!("cleanup.order_by_size")
                            } else {
                                i18n::t!("cleanup.order_by_age")
                            },
                            true,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_cleanup_order(cx);
                            }),
                        ),
                    )
                    .child(
                        button(
                            "cleanup-select-finished",
                            i18n::t!("cleanup.select_finished"),
                            true,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.select_finished_cleanup_rows(cx);
                            }),
                        ),
                    )
                    .child(
                        div()
                            .id("cleanup-refresh")
                            .px(px(6.))
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3))
                            .child("↻")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.count_cleanup_stakes(cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("cleanup-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(actions)
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("cleanup.hint"))),
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
                .bg(gpui::hsla(0., 0., 0., 0.35))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_cleanup(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

/// 失うものを数える（背景で呼ぶ）。比較先は先に解決できた物で数える。
fn measure_stakes(host: &dyn host::Host, root: &Path, bases: &[String]) -> WorktreeStakes {
    WorktreeStakes {
        dirty_files: project::git_status_on(host, root).len(),
        unmerged_commits: bases
            .iter()
            .find_map(|base| project::git_unmerged_count_on(host, root, base)),
    }
}

/// 失うものが無ければ worktree を消す（`Ok(true)`）。数え直して何かあれば消さない（`Ok(false)`）。
/// 消すのはメインの作業ツリーから（`git worktree remove --force`・ブランチは残す）。
fn delete_if_nothing_to_lose(
    host: &dyn host::Host,
    root: &Path,
    bases: &[String],
) -> anyhow::Result<bool> {
    if !measure_stakes(host, root, bases).is_recoverable() {
        return Ok(false);
    }
    let main = project::git_worktrees_on(host, root)
        .into_iter()
        .map(|worktree| worktree.path)
        .find(|path| path != root)
        .ok_or_else(|| anyhow::anyhow!(i18n::t!("explorer.main_worktree_undeletable")))?;
    project::remove_worktree_on(host, &main, root, true)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::LocalHost;

    /// 実 git: 失うものが無い worktree だけ消え、未コミットの変更がある物は残る。
    #[test]
    fn only_worktrees_with_nothing_to_lose_are_deleted() {
        let base = std::env::temp_dir().join(format!("necoder_cleanup_{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).expect("作れる");
        let git = |dir: &Path, args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(dir)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .expect("git を起動できる")
        };
        if !git(&repo, &["init", "-q", "-b", "main"]).status.success() {
            return;
        }
        std::fs::write(repo.join("a.txt"), "1\n").expect("書ける");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "first"]);
        let clean = base.join("clean");
        let dirty = base.join("dirty");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "task/clean",
                clean.to_str().expect("utf8"),
            ],
        );
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "task/dirty",
                dirty.to_str().expect("utf8"),
            ],
        );
        std::fs::write(dirty.join("a.txt"), "changed\n").expect("書ける");
        let bases = vec!["main".to_string()];

        assert!(measure_stakes(&LocalHost, &clean, &bases).is_recoverable());
        assert!(!measure_stakes(&LocalHost, &dirty, &bases).is_recoverable());
        assert!(delete_if_nothing_to_lose(&LocalHost, &clean, &bases).expect("消せる"));
        assert!(!clean.exists(), "消えた");
        assert!(
            !delete_if_nothing_to_lose(&LocalHost, &dirty, &bases).expect("判定できる"),
            "未コミットの変更がある物は消さない"
        );
        assert!(dirty.join("a.txt").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    /// 画面: いまのリポジトリの Task だけを並べ、統合済みをまとめて選んで終了できる。
    #[gpui::test]
    fn finished_tasks_can_be_selected_and_ended_together(cx: &mut gpui::TestAppContext) {
        let base =
            std::env::temp_dir().join(format!("necoder_cleanup_view_{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let folders: Vec<PathBuf> = ["main", "done", "working"]
            .iter()
            .map(|name| base.join(name))
            .collect();
        for folder in &folders {
            std::fs::create_dir_all(folder).expect("作れる");
        }
        let settings_path = base.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(folders.clone(), Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            for slot in &mut workspace.project_sessions.projects {
                slot.task_space.repository_id = "repo".to_string();
            }
            for (index, phase) in [(1, TaskPhase::Integrated), (2, TaskPhase::Working)] {
                let slot = &mut workspace.project_sessions.projects[index];
                slot.task_space.kind = SpaceKind::Task;
                slot.task_space.phase = phase;
            }
            workspace.switch_project(0, window, cx);
            workspace.show_cleanup(&ShowCleanup, window, cx);
            let state = workspace.overlays.cleanup.as_ref().expect("開く");
            assert_eq!(state.rows.len(), 2, "統合先は並べない");

            workspace.select_finished_cleanup_rows(cx);
            let done = workspace.project_sessions.projects[1].task_space.id.clone();
            let selected = &workspace
                .overlays
                .cleanup
                .as_ref()
                .expect("開いている")
                .selected;
            assert_eq!(selected.len(), 1);
            assert!(selected.contains(&done));

            workspace.archive_selected_cleanup_rows(cx);
            assert_eq!(
                workspace.project_sessions.projects[1].task_space.phase,
                TaskPhase::Archived
            );
            assert_eq!(
                workspace.project_sessions.projects[2].task_space.phase,
                TaskPhase::Working,
                "選んでいない Task は触らない"
            );
        });
        // 大きさは背景で測って行に入る（O22）。大きい順に並べ替えられる。
        std::fs::write(folders[2].join("big.bin"), vec![0u8; 256 * 1024]).expect("書ける");
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.count_cleanup_stakes(cx)
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            let state = workspace.overlays.cleanup.as_ref().expect("開いている");
            let working = workspace.project_sessions.projects[2].task_space.id.clone();
            let big = state
                .rows
                .iter()
                .find(|row| row.space == working)
                .and_then(|row| row.size)
                .expect("測れた");
            assert!(big >= 256 * 1024, "{big}");
            assert_eq!(
                cleanup_order(&state.rows, true)
                    .first()
                    .map(|&index| state.rows[index].space.clone()),
                Some(working),
                "大きい順なら先頭"
            );
            workspace.toggle_cleanup_order(cx);
            assert!(workspace
                .overlays
                .cleanup
                .as_ref()
                .is_some_and(|state| state.by_size));
        });
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn sizes_read_in_the_nearest_unit() {
        assert_eq!(size_label(None), "—");
        assert_eq!(size_label(Some(12 * 1024)), "12 KB");
        assert_eq!(size_label(Some(340 * 1024 * 1024)), "340 MB");
        assert_eq!(size_label(Some(1_288_490_189)), "1.2 GB");
    }
}
