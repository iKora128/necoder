use crate::workspace::*;
use gpui::{canvas, point, relative, PathBuilder};

/// 編隊ビューの 1 レーン（= 1 スレッド）。系譜グラフ・グリッド・ニュースが共通で読む owned データ。
struct FleetLane {
    space_index: usize,
    thread_index: usize,
    branch: Option<SharedString>,
    color: Hsla,
    activity: agent_panel::ThreadActivity,
    name: SharedString,
    agent: SharedString,
    tokens_used: u32,
    tokens_max: u32,
    /// review 完了ではなく IntegrationSpace へ実際に merge 済みか。
    integrated: bool,
    /// base から HEAD が進んでいるか。固定のダミー commit bead を描かないための実 OID 差分。
    has_commits: bool,
}

/// 系譜グラフの 1 枝（= 1 スレッド）。本流の分岐点 `split` から cubic で `elbow` へ下り、
/// そこから水平に `tip`（先端エージェント）へ至る。`beads` は水平区間に並ぶコミット丸（中空）。
/// 座標はすべて body の割合（0..1）。tip は `activity_dot`（色＝帰属・形＝状態）で描く。
struct GraphBranch {
    split: (f32, f32),
    control1: (f32, f32),
    control2: (f32, f32),
    elbow: (f32, f32),
    tip: (f32, f32),
    beads: Vec<(f32, f32)>,
    /// 完了枝が本流へ戻る合流カーブの終点（扇形の Done のみ・`None`＝合流なし）。
    merge: Option<(f32, f32)>,
    color: Hsla,
    activity: agent_panel::ThreadActivity,
    dashed: bool,
    name: SharedString,
    branch: Option<SharedString>,
    agent: SharedString,
    /// クリックで focus する先（レール space × スレッド）。`fleet_lanes` の座標に一致。
    space: usize,
    thread: usize,
}

/// canvas に渡す枝の背骨（`GraphBranch` の幾何だけを owned・Copy で持つ）。overlay と別に
/// 描くため、SharedString を含まない座標＋色に絞る。座標は body に対する割合（0..1）。
#[derive(Clone, Copy)]
struct Spine {
    split: (f32, f32),
    control1: (f32, f32),
    control2: (f32, f32),
    elbow: (f32, f32),
    tip: (f32, f32),
    /// 完了枝 → 本流の合流カーブ終点（`None`＝合流なし）。
    merge: Option<(f32, f32)>,
    color: Hsla,
    dashed: bool,
}

/// 系譜グラフ 1 枚分の版面（扇形/ツリーで共通・カード/ハブは別描画）。座標は割合（0..1）。
/// 本流 main は水平（扇形）または縦（ツリーの root→split）で、枝がそこから分岐する。
struct GraphScene {
    branches: Vec<GraphBranch>,
    main_y: f32,
    /// 本流の実線区間 [start, split]。扇形では split から先が破線（＝統合待ち）。
    trunk_start_x: f32,
    trunk_split_x: f32,
    trunk_end_x: f32,
    /// 本流上のコミットドット（扇形のみ・中空グレー）。リバー/ツリーは空。
    base_commits: Vec<(f32, f32)>,
    /// main / HEAD ラベルの位置（扇形/リバー）。
    head_label: (f32, f32),
    /// ツリーの root ノード（`Some`＝ツリー）。`main_y..split` に短い縦幹を描く。
    root: Option<(f32, f32)>,
    /// root 上のキャプション（"base a3f9c1 ・ N エージェント並走"）。
    caption: Option<SharedString>,
    /// ラベルをノードの下（中央寄せ）に置くか（ツリー）。false＝右に置く（扇形/リバー）。
    labels_below: bool,
}

/// plan 進捗メーター（P1・mock `fleet-dashboard.html` の ▰▱ 書式）。5 セグメントに丸めて
/// 「▰▰▰▱▱ 3/5」を返す。**状態に色相を使わない**規律どおり文字グリフで見せる（fg2 で描く）。
pub(crate) fn plan_meter(done: u32, total: u32) -> SharedString {
    if total == 0 {
        return SharedString::default();
    }
    let filled = ((done as f32 / total as f32) * 5.0).round() as usize;
    let filled = filled.min(5);
    let meter: String = "▰".repeat(filled) + &"▱".repeat(5 - filled);
    SharedString::from(format!("{meter} {done}/{total}"))
}

/// body 矩形に対する割合座標 (xf,yf) を中心に、直径 `diameter` の円 overlay の土台 Div を返す。
/// 左を割合、負マージンで半径ぶん戻して中央合わせ。呼び出し側が bg/border を足してコミット丸にする。
fn dot_overlay(xf: f32, yf: f32, diameter: f32, body_h: f32) -> gpui::Div {
    div()
        .absolute()
        .left(relative(xf))
        .ml(px(-diameter / 2.0))
        .top(px(yf * body_h - diameter / 2.0))
        .size(px(diameter))
        .flex_none()
        .rounded_full()
}

impl Workspace {
    pub(crate) fn toggle_fleet_mode(
        &mut self,
        _: &ToggleFleet,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chrome.fleet_mode = !self.chrome.fleet_mode;
        if self.chrome.fleet_mode {
            if self.chrome.fleet_cells.is_empty() {
                self.seed_fleet_cells(cx);
            }
            self.ensure_fleet_clock(cx); // 相対時刻（開始/入力）を古びさせない
        }
        cx.notify();
    }

    /// 編隊 / herd の相対時刻表示（「N分前」）を古びさせない 30 秒時計。どちらかが見えている間だけ
    /// 回り、両方閉じたら次の tick で自停止する（idle 予算を守る。多重起動は `fleet_clock` で防ぐ）。
    pub(crate) fn ensure_fleet_clock(&mut self, cx: &mut Context<Self>) {
        if self.chrome.fleet_clock {
            return;
        }
        self.chrome.fleet_clock = true;
        cx.spawn(async move |workspace, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                let keep_running = workspace.update(cx, |workspace, cx| {
                    if workspace.chrome.fleet_mode || workspace.chrome.show_herd {
                        cx.notify();
                        true
                    } else {
                        workspace.chrome.fleet_clock = false;
                        false
                    }
                });
                if !matches!(keep_running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// 編隊グリッドを TaskSpace（= linked worktree）単位で配置する。
    /// IntegrationSpace は保護された統合先なので Agent セルにはせず、herd / lineage にだけ残す。
    pub(crate) fn seed_fleet_cells(&mut self, cx: &App) {
        self.chrome.fleet_cells = self
            .project_sessions
            .projects
            .iter()
            .filter(|slot| !slot.task_space.is_integration())
            .take(8)
            .map(|slot| FleetPane::Task { space: slot.task_space.id.clone() })
            .collect();
        // 開発用: 最初の Task の実端末 surface を 1 枚仕込む。
        if std::env::var_os("SHIRUSHI_FLEET_TERM").is_some() && self.chrome.fleet_cells.len() < 8 {
            if let Some(space) = self.chrome.fleet_cells.iter().find_map(|pane| match pane {
                FleetPane::Task { space } => Some(space.clone()),
                _ => None,
            }) {
                self.chrome.fleet_cells.push(FleetPane::Terminal { space });
            }
        }
        let _ = cx;
    }

    #[allow(dead_code)] // 2026-07-24 の 1 択化で入口を外した（機能は保持）
    fn add_fleet_cell(&mut self, pane: FleetPane, cx: &mut Context<Self>) {
        if let Some(index) = self.chrome.fleet_cells.iter().position(|current| current == &pane) {
            self.chrome.fleet_maximized = Some(index);
        } else if self.chrome.fleet_cells.len() < 8 {
            self.chrome.fleet_cells.push(pane);
        }
        cx.notify();
    }

    #[allow(dead_code)] // 2026-07-24 の 1 択化で入口を外した（機能は保持）
    fn add_terminal_to_selected_task(&mut self, cx: &mut Context<Self>) {
        let Some(space) = self.selected_task_space() else {
            return;
        };
        if let Some(index) = self.session_index_for_space(&space) {
            self.project_sessions.sessions[index]
                .terminal_dock
                .update(cx, |dock, cx| {
                    dock.ensure_active(cx);
                });
        }
        self.add_fleet_cell(FleetPane::Terminal { space }, cx);
    }

    #[allow(dead_code)] // 2026-07-24 の 1 択化で入口を外した（機能は保持）
    fn add_editor_to_selected_task(&mut self, cx: &mut Context<Self>) {
        if let Some(space) = self.selected_task_space() {
            self.add_fleet_cell(FleetPane::Editor { space }, cx);
        }
    }

    #[allow(dead_code)] // 2026-07-24 の 1 択化で入口を外した（機能は保持）
    fn add_diff_to_selected_task(&mut self, cx: &mut Context<Self>) {
        if let Some(space) = self.selected_task_space() {
            if let Some(index) = self.session_index_for_space(&space) {
                self.refresh_git_status_for(index, cx);
            }
            self.add_fleet_cell(FleetPane::Diff { space }, cx);
        }
    }

    #[allow(dead_code)] // 2026-07-24 の 1 択化で入口を外した（機能は保持）
    fn add_tests_to_selected_task(&mut self, cx: &mut Context<Self>) {
        let Some(space) = self.selected_task_space() else {
            return;
        };
        if let Some(index) = self.session_index_for_space(&space) {
            self.project_sessions.sessions[index]
                .tests_dock
                .update(cx, |dock, cx| {
                    dock.ensure_active(cx);
                });
        }
        self.add_fleet_cell(FleetPane::Tests { space }, cx);
    }

    /// Review gate: worktree を変更しない merge-tree preview で conflict を先に検出する。
    pub(crate) fn review_task_for_merge(&mut self, space: SpaceId, cx: &mut Context<Self>) {
        let Some(task_index) = self.session_index_for_space(&space) else {
            return;
        };
        let task = &self.project_sessions.projects[task_index];
        let Some(branch) = task.branch.clone().or_else(|| task.worktree_branch.clone()) else {
            return;
        };
        let repository_id = task.task_space.repository_id.clone();
        let Some(integration) = self.project_sessions.projects.iter().find(|slot| {
            slot.task_space.repository_id == repository_id && slot.task_space.is_integration()
        }) else {
            self.push_toast("IntegrationSpace が開かれていません".into(), self.accent(), cx);
            return;
        };
        let host = integration.worktree.host().clone();
        let root = integration.worktree.root().to_path_buf();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::preview_merge_on(host.as_ref(), &root, &branch) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match result {
                Ok(preview) if preview.clean => {
                    workspace.transition_task_space(
                        task_index,
                        TaskPhase::MergeReady,
                        "merge_preview_clean",
                        Some("Conflict Radar: clean"),
                        cx,
                    );
                    workspace.push_toast("Conflict Radar: 統合可能です".into(), workspace.accent(), cx);
                }
                Ok(preview) => {
                    workspace.transition_task_space(
                        task_index,
                        TaskPhase::ChangesRequested,
                        "merge_preview_conflict",
                        Some(&preview.detail),
                        cx,
                    );
                    workspace.push_toast(
                        SharedString::from(format!("Conflict Radar: {}", preview.detail)),
                        workspace.accent(),
                        cx,
                    );
                }
                Err(error) => workspace.push_toast(
                    SharedString::from(format!("merge preview に失敗: {error:#}")),
                    workspace.accent(),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// 明示操作だけが IntegrationSpace を更新する。dirty / conflict は project 層で拒否される。
    pub(crate) fn integrate_task(&mut self, space: SpaceId, cx: &mut Context<Self>) {
        let Some(task_index) = self.session_index_for_space(&space) else {
            return;
        };
        let task = &self.project_sessions.projects[task_index];
        if task.task_space.phase != TaskPhase::MergeReady {
            return;
        }
        let Some(branch) = task.branch.clone().or_else(|| task.worktree_branch.clone()) else {
            return;
        };
        let repository_id = task.task_space.repository_id.clone();
        let Some(integration_index) = self.project_sessions.projects.iter().position(|slot| {
            slot.task_space.repository_id == repository_id && slot.task_space.is_integration()
        }) else {
            return;
        };
        let integration = &self.project_sessions.projects[integration_index];
        let host = integration.worktree.host().clone();
        let root = integration.worktree.root().to_path_buf();
        self.transition_task_space(task_index, TaskPhase::Integrating, "integration_started", None, cx);
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::integrate_branch_on(host.as_ref(), &root, &branch) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match result {
                Ok(head_oid) => {
                    if let Some(slot) = workspace.project_sessions.projects.get_mut(integration_index) {
                        slot.task_space.head_oid = Some(head_oid);
                    }
                    workspace.persist_task_space(integration_index, cx);
                    workspace.transition_task_space(
                        task_index,
                        TaskPhase::Integrated,
                        "integration_succeeded",
                        None,
                        cx,
                    );
                    workspace.refresh_git_status_for(integration_index, cx);
                    workspace.push_toast("Task を IntegrationSpace へ統合しました".into(), workspace.accent(), cx);
                }
                Err(error) => {
                    workspace.transition_task_space(
                        task_index,
                        TaskPhase::MergeReady,
                        "integration_failed",
                        Some(&format!("{error:#}")),
                        cx,
                    );
                    workspace.push_toast(
                        SharedString::from(format!("統合できません: {error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub(crate) fn session_index_for_space(&self, space: &SpaceId) -> Option<usize> {
        self.project_sessions
            .projects
            .iter()
            .position(|slot| &slot.task_space.id == space)
    }

    fn selected_task_space(&self) -> Option<SpaceId> {
        self.chrome
            .fleet_maximized
            .and_then(|index| self.chrome.fleet_cells.get(index))
            .and_then(|pane| match pane {
                FleetPane::Task { space }
                | FleetPane::Terminal { space }
                | FleetPane::Editor { space }
                | FleetPane::Diff { space }
                | FleetPane::Tests { space } => Some(space.clone()),
            })
            .or_else(|| {
                self.project_sessions
                    .projects
                    .get(self.project_sessions.active)
                    .filter(|slot| !slot.task_space.is_integration())
                    .map(|slot| slot.task_space.id.clone())
            })
            .or_else(|| {
                self.chrome.fleet_cells.iter().find_map(|pane| match pane {
                    FleetPane::Task { space } => Some(space.clone()),
                    _ => None,
                })
            })
    }

    /// `+ Agent to this Task`: 選択 TaskSpace の完全な AgentPanel に thread を追加する。
    /// surface は増やさないため、1 worktree / 1 composer の ownership を破らない。
    pub(crate) fn add_fleet_agent(&mut self, cx: &mut Context<Self>) {
        let Some(space) = self.selected_task_space() else {
            self.push_toast(
                SharedString::from("先に ＋ Task で隔離 worktree を作成してください"),
                self.accent(),
                cx,
            );
            return;
        };
        let Some(session_index) = self.session_index_for_space(&space) else {
            return;
        };
        let panel = self.project_sessions.sessions[session_index].agent_panel.clone();
        panel.update(cx, |panel, cx| panel.new_thread(cx));
        cx.notify();
    }

    /// Fleet の既定操作は設定に左右されず、常に隔離 Task を作る。
    fn add_fleet_agent_default(&mut self, cx: &mut Context<Self>) {
        self.add_worktree_agent(cx);
    }

    /// `+ Task`: IntegrationSpace の HEAD から `task/<n>` branch + linked worktree を作り、
    /// ProjectSession/AgentPanel を 1 組起動する。Fleet の通常作業は必ずこの隔離導線を通る。
    pub(crate) fn add_worktree_agent(&mut self, cx: &mut Context<Self>) {
        if self.chrome.fleet_cells.len() >= 8 {
            return;
        }
        let active_repository = self.active_slot().map(|slot| slot.task_space.repository_id.clone());
        let integration_index = active_repository
            .as_ref()
            .and_then(|repository_id| {
                self.project_sessions.projects.iter().position(|slot| {
                    slot.task_space.repository_id == *repository_id && slot.task_space.is_integration()
                })
            })
            .unwrap_or(self.project_sessions.active);
        let Some(slot) = self.project_sessions.projects.get(integration_index) else {
            return;
        };
        let worktree = slot.worktree.clone();
        let root = worktree.root().to_path_buf();
        let repo_name = root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string());
        let Some(parent) = root.parent().map(Path::to_path_buf) else {
            return;
        };
        // branch = task/<n>（レールで未使用の最小 n）。
        let open_branches: std::collections::HashSet<String> = self
            .project_sessions
            .projects
            .iter()
            .filter_map(|slot| slot.branch.clone().or_else(|| slot.worktree_branch.clone()))
            .collect();
        let host = worktree.host().clone();
        let host_for_add = host.clone();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut used = open_branches;
                    used.extend(project::git_branches_on(host_for_add.as_ref(), &root));
                    let mut number = 1;
                    let (branch, target) = loop {
                        let branch = format!("task/{number}");
                        let target = parent.join(format!("{repo_name}-task-{number}"));
                        if !used.contains(&branch) && host_for_add.metadata(&target).is_err() {
                            break (branch, target);
                        }
                        number += 1;
                    };
                    project::create_task_worktree_on(
                        host_for_add.as_ref(),
                        &root,
                        &target,
                        &branch,
                    )?;
                    Ok::<(PathBuf, String), anyhow::Error>((target, branch))
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match result {
                Ok((target, branch)) => {
                    // worktree を space（レール slot）として開く（switch も走る）。
                    workspace.open_folder_in_rail(host, target.clone(), Some(branch), cx);
                    // その TaskSpace の AgentPanel に新スレッドを起動 → Task セルを足す。
                    if let Some(space) = workspace
                        .project_sessions
                        .projects
                        .iter()
                        .position(|slot| slot.worktree.root() == target.as_path())
                    {
                        let space_id = workspace.project_sessions.projects[space].task_space.id.clone();
                        if workspace.chrome.fleet_cells.len() < 8 {
                            workspace.chrome.fleet_cells.push(FleetPane::Task { space: space_id });
                        }
                        workspace.persist_task_space(space, cx);
                        workspace.transition_task_space(
                            space,
                            TaskPhase::Planned,
                            "task_created",
                            None,
                            cx,
                        );
                    }
                    cx.notify();
                }
                Err(error) => {
                    let accent = workspace.accent();
                    workspace.push_toast(SharedString::from(format!("{error:#}")), accent, cx);
                }
            });
        })
        .detach();
    }

    fn close_fleet_cell(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.chrome.fleet_cells.len() {
            self.chrome.fleet_cells.remove(index);
            // 拡大中のセルを閉じたら拡大解除・後ろを閉じたら添字を詰める。
            match self.chrome.fleet_maximized {
                Some(max) if max == index => self.chrome.fleet_maximized = None,
                Some(max) if max > index => self.chrome.fleet_maximized = Some(max - 1),
                _ => {}
            }
            cx.notify();
        }
    }

    /// エージェント（スレッド）を閉じる。Task cell は worktree の surface なので維持する。
    pub(crate) fn close_agent(&mut self, space: usize, thread: usize, cx: &mut Context<Self>) {
        let Some(session) = self.project_sessions.sessions.get(space) else {
            return;
        };
        session
            .agent_panel
            .clone()
            .update(cx, |panel, cx| panel.close_thread(thread, cx));
        cx.notify();
    }

    /// セルを拡大表示（mock の focus・M14）。系譜グラフを隠し、そのセルを大きく・他はサムネイル列へ。
    fn maximize_fleet_cell(&mut self, index: usize, cx: &mut Context<Self>) {
        self.chrome.fleet_maximized = Some(index);
        cx.notify();
    }

    /// 拡大表示を解除して通常のグリッド（系譜グラフ + N分割）へ戻す。
    fn restore_fleet_layout(&mut self, cx: &mut Context<Self>) {
        self.chrome.fleet_maximized = None;
        cx.notify();
    }

    /// 左 Fleet 列のエージェントをグリッドに出す（無ければセル追加）→ 拡大表示（M14）。
    /// ＝タップで必ず反応し、× で閉じたセルもここから復帰できる。
    pub(crate) fn reveal_agent_in_fleet(
        &mut self,
        space: usize,
        thread: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(space) else {
            return;
        };
        // IntegrationSpace（main）もセルに出せる（2026-07-24 ユーザー指摘で「保護」トーストを撤廃。
        // 本当に守るべきは Git 側 = 統合の radar + 人間 gate と台帳の遷移拒否で、画面の取り締まりではない）。
        let space_id = slot.task_space.id.clone();
        let target = FleetPane::Task { space: space_id.clone() };
        if let Some(session) = self.project_sessions.sessions.get(space) {
            session.agent_panel.update(cx, |panel, cx| panel.focus_thread(thread, cx));
        }
        self.switch_project(space, window, cx);
        let index = match self.chrome.fleet_cells.iter().position(|pane| pane == &target) {
            Some(index) => index,
            None => {
                if self.chrome.fleet_cells.len() >= 8 {
                    return;
                }
                self.chrome.fleet_cells.push(target);
                self.chrome.fleet_cells.len() - 1
            }
        };
        self.chrome.fleet_maximized = Some(index);
        cx.notify();
    }

    fn set_graph_view(&mut self, view: GraphView, cx: &mut Context<Self>) {
        self.chrome.graph_view = view;
        self.chrome.graph_collapsed = false;
        cx.notify();
    }

    fn toggle_graph_collapse(&mut self, cx: &mut Context<Self>) {
        self.chrome.graph_collapsed = !self.chrome.graph_collapsed;
        cx.notify();
    }

    /// 1 TaskSpace = 1 lineage branch。Task 内の複数 AgentRun は最重要 runtime 状態へロールアップし、
    /// main 上の通常 View thread を「別 worktree の枝」として誤描画しない。
    fn fleet_lanes(&self, cx: &App) -> Vec<FleetLane> {
        let mut lanes = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if slot.task_space.is_integration() {
                continue;
            }
            if slot.task_space.phase == TaskPhase::Archived {
                continue; // アーカイブ済みはグラフ/セルにも出さない（3 層の中段・2026-07-24）
            }
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            let branch = slot
                .branch
                .clone()
                .or_else(|| slot.worktree_branch.clone())
                .map(SharedString::from);
            let statuses = session.agent_panel.read(cx).statuses();
            let (thread_index, representative) = statuses
                .iter()
                .enumerate()
                .max_by_key(|(_, status)| status.activity.urgency())
                .map(|(index, status)| (index, Some(status)))
                .unwrap_or((0, None));
            let runtime_activity = representative
                .map(|status| status.activity)
                .unwrap_or(agent_panel::ThreadActivity::Idle);
            let activity = match slot.task_space.phase {
                TaskPhase::Working => agent_panel::ThreadActivity::Working,
                TaskPhase::Blocked => agent_panel::ThreadActivity::Blocked,
                TaskPhase::ReviewReady | TaskPhase::MergeReady | TaskPhase::Integrated => {
                    agent_panel::ThreadActivity::Done { interrupted: false }
                }
                TaskPhase::ChangesRequested | TaskPhase::Failed => {
                    agent_panel::ThreadActivity::Done { interrupted: true }
                }
                _ => runtime_activity,
            };
            lanes.push(FleetLane {
                space_index: index,
                thread_index,
                branch,
                color: slot.color,
                activity,
                name: slot.task_space.title.clone(),
                agent: representative
                    .map(|status| status.agent.clone())
                    .unwrap_or_else(|| SharedString::from("Agent")),
                tokens_used: statuses.iter().map(|status| status.tokens_used).sum(),
                tokens_max: statuses.iter().map(|status| status.tokens_max).sum(),
                integrated: slot.task_space.phase == TaskPhase::Integrated,
                has_commits: matches!(
                    (&slot.task_space.base_oid, &slot.task_space.head_oid),
                    (Some(base), Some(head)) if base != head
                ),
            });
            if lanes.len() >= 8 {
                return lanes;
            }
        }
        lanes
    }

    /// 編隊ビュー本体（mock の編隊モード）: レール | herd サイドバー | 中央（系譜グラフ + グリッド + ニュース）。
    /// セル拡大中（`fleet_maximized`）は系譜グラフを隠し、拡大セル + サムネイル列に切替（ユーザー案）。
    pub(crate) fn render_fleet(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let maximized = self
            .chrome
            .fleet_maximized
            .filter(|index| *index < self.chrome.fleet_cells.len());
        let mut center = div().flex_1().flex().flex_col().min_w_0().bg(theme.bg1);
        // 中央タブ帯（P3）: 管制 / グラフ。セル拡大中は没入を優先してタブ帯を出さない。
        if maximized.is_none() {
            center = center.child(self.render_center_tabs(cx));
        }
        center = match maximized {
            Some(index) => {
                // 拡大: 上にサムネイル列（他セルが避ける）+ 大きい拡大セル（グラフの場所に出る）。
                let lanes = self.fleet_lanes(cx);
                center
                    .child(self.render_fleet_thumbnails(index, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p(px(10.))
                            .child(self.render_fleet_cell(
                                index,
                                self.chrome.fleet_cells[index].clone(),
                                &lanes,
                                true,
                                cx,
                            )),
                    )
            }
            None => match self.chrome.fleet_center_view {
                FleetCenterView::Control => center.child(self.render_control(cx)),
                FleetCenterView::Graph => center
                    .child(self.render_lineage_graph(cx))
                    .child(self.render_fleet_grid(cx)),
            },
        };
        center = center.child(self.render_newsfeed(cx));
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.render_rail(cx))
            .child(self.render_herd_sidebar(cx))
            .child(center)
            .into_any_element()
    }

    /// 拡大表示中のサムネイル列（mock focus 時の他セル）。全セルを小さく横並びに・クリックで拡大先を切替。
    fn render_fleet_thumbnails(&self, max_index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let cells = self.chrome.fleet_cells.clone();
        let mut strip = div()
            .id("fleet-thumbs")
            .flex_none()
            .h(px(72.))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.))
            .overflow_x_scroll()
            .border_b_1()
            .border_color(theme.border);
        for (index, pane) in cells.iter().enumerate() {
            let space = match pane {
                FleetPane::Task { space }
                | FleetPane::Terminal { space }
                | FleetPane::Editor { space }
                | FleetPane::Diff { space }
                | FleetPane::Tests { space } => space,
            };
            let session_index = self.session_index_for_space(space);
            let slot = session_index.and_then(|index| self.project_sessions.projects.get(index));
            let color = slot.map(|slot| slot.color).unwrap_or(theme.fg2);
            let activity = session_index.and_then(|session_index| {
                self.project_sessions.sessions[session_index]
                    .agent_panel
                    .read(cx)
                    .statuses()
                    .into_iter()
                    .max_by_key(|status| status.activity.urgency())
                    .map(|status| status.activity)
            });
            let surface = match pane {
                FleetPane::Task { .. } => "Agent",
                FleetPane::Terminal { .. } => "Terminal",
                FleetPane::Editor { .. } => "Editor",
                FleetPane::Diff { .. } => "Diff",
                FleetPane::Tests { .. } => "Tests",
            };
            let name = slot
                .map(|slot| SharedString::from(format!("{} · {surface}", slot.task_space.title)))
                .unwrap_or_else(|| SharedString::from(surface));
            let branch = slot.and_then(|slot| {
                slot.branch
                    .clone()
                    .or_else(|| slot.worktree_branch.clone())
                    .map(SharedString::from)
            });
            let active = index == max_index;
            let sub = match &branch {
                Some(branch) => SharedString::from(format!("⎇ {branch}")),
                None => SharedString::from(surface),
            };
            strip = strip.child(
                div()
                    .id(("fleet-thumb", index))
                    .w(px(184.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .px(px(9.))
                    .py(px(6.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(if active { accent } else { theme.border })
                    .bg(if active { theme.bg2 } else { theme.bg0 })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().size(px(7.)).rounded_full().bg(color).flex_none())
                            .when_some(activity, |element, activity| {
                                element.child(agent_panel::activity_dot(("thumb-dot", index), 8.0, color, activity))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.fg0)
                                    .child(name),
                            ),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(sub),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.maximize_fleet_cell(index, cx)),
                    ),
            );
        }
        strip.into_any_element()
    }

    // ── 系譜グラフ（M14 #4・最重要ビジュアル・ネイティブ描画） ──

    /// ヘッダ: 「系譜グラフ」 + 4 表示スイッチャー（扇形/リバー/ツリー/カード）+ ⌄ 折り畳み。
    fn render_graph_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let current = self.chrome.graph_view;
        let collapsed = self.chrome.graph_collapsed;
        let views = [
            (GraphView::Fan, i18n::t!("fleet.graph_fan")),
            (GraphView::Tree, i18n::t!("fleet.graph_tree")),
            (GraphView::Card, i18n::t!("fleet.graph_card")),
            (GraphView::Hub, i18n::t!("fleet.graph_hub")),
        ];
        let mut switcher = div().flex().items_center().gap(px(2.));
        for (index, (view, label)) in views.into_iter().enumerate() {
            let on = view == current;
            switcher = switcher.child(
                div()
                    .id(("graph-view", index))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(5.))
                    .text_size(px(10.5))
                    .when(on, |element| element.bg(theme.bg3))
                    .text_color(if on { theme.fg0 } else { theme.fg2 })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg1))
                    .child(SharedString::from(label))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.set_graph_view(view, cx)),
                    ),
            );
        }
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(30.))
            .px(px(12.))
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg2)
                    .child(i18n::t!("fleet.lineage")),
            )
            .child(div().flex_1())
            .child(switcher)
            .child(
                div()
                    .id("graph-collapse")
                    .flex_none()
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg1))
                    .child(SharedString::from(if collapsed { "▸" } else { "⌄" }))
                    .tooltip(Tooltip::text(i18n::t!("fleet.graph_collapse"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.toggle_graph_collapse(cx)),
                    ),
            )
            // スイッチャーがプロジェクト色に馴染むよう、アクティブ表示だけ僅かに accent 寄せ（下線）。
            .border_b_1()
            .border_color(if collapsed { theme.border } else { accent.alpha(0.0) })
    }

    /// 扇形（系譜）の版面。本流の分岐点から枝が出て、肘で水平化し、コミット bead を経て先端に至る。
    /// 本流は 3 コミット + 破線の統合待ち尾を持ち、完了枝は main へ戻る合流カーブを引く。
    fn radial_scene(&self, lanes: &[FleetLane]) -> GraphScene {
        let count = lanes.len().max(1);
        let main_y = 0.13_f32;
        let trunk_start_x = 0.03_f32;
        let split_x = 0.17_f32;
        let lane_left = split_x + 0.15; // 枝が水平になる x（肘）
        let tip_x = 0.50_f32;
        let trunk_end_x = 0.86_f32;
        let (row_top, row_span) = (0.30_f32, 0.60_f32);
        let mut branches = Vec::new();
        for (index, lane) in lanes.iter().enumerate() {
            let yf = row_top + (index as f32 + 0.5) / count as f32 * row_span;
            let idle = matches!(lane.activity, agent_panel::ThreadActivity::Idle);
            // idle はコミット無しで破線・稼働中は水平区間に中空 bead を 2 個置く。
            let beads = if idle || !lane.has_commits {
                Vec::new()
            } else {
                vec![((lane_left + tip_x) / 2.0, yf)]
            };
            // 完了枝は本流へ戻る合流カーブを引く（mock の「⤳ main」）。ラベル帯より右で合流。
            let merge = if lane.integrated { Some((0.80_f32, main_y)) } else { None };
            branches.push(GraphBranch {
                split: (split_x, main_y),
                control1: (split_x + 0.06, main_y),
                control2: (lane_left - 0.05, yf),
                elbow: (lane_left, yf),
                tip: (tip_x, yf),
                beads,
                merge,
                color: lane.color,
                activity: lane.activity,
                dashed: idle,
                name: lane.name.clone(),
                branch: lane.branch.clone(),
                agent: lane.agent.clone(),
                space: lane.space_index,
                thread: lane.thread_index,
            });
        }
        // Task の `base_oid` を表す 1 anchor。履歴数を知らないのに固定 3 commit を捏造しない。
        let base_commits = vec![((trunk_start_x + split_x) / 2.0, main_y)];
        GraphScene {
            branches,
            main_y,
            trunk_start_x,
            trunk_split_x: split_x,
            trunk_end_x,
            base_commits,
            head_label: (trunk_end_x + 0.006, main_y),
            root: None,
            caption: None,
            labels_below: false,
        }
    }

    /// ツリーの版面。root から短い縦幹が下り、枝が下方へ扇状に分かれて各ノード（先端）に至る。
    /// ラベルはノードの下に中央寄せ。root 上に "base ・ N エージェント並走" のキャプションを出す。
    fn tree_scene(&self, lanes: &[FleetLane]) -> GraphScene {
        let count = lanes.len().max(1);
        let root = (0.5_f32, 0.16_f32);
        let split = (0.5_f32, 0.34_f32);
        let node_y = 0.72_f32;
        let mid_y = (split.1 + node_y) / 2.0;
        let (col_left, col_span) = (0.12_f32, 0.76_f32);
        let mut branches = Vec::new();
        for (index, lane) in lanes.iter().enumerate() {
            let xf = col_left + (index as f32 + 0.5) / count as f32 * col_span;
            let idle = matches!(lane.activity, agent_panel::ThreadActivity::Idle);
            let beads = if idle || !lane.has_commits {
                Vec::new()
            } else {
                vec![(xf, node_y - 0.13)]
            };
            branches.push(GraphBranch {
                split,
                control1: (split.0, mid_y),
                control2: (xf, mid_y),
                elbow: (xf, node_y), // ツリーは水平区間なし（elbow == tip）
                tip: (xf, node_y),
                beads,
                merge: None, // ツリーは合流を描かない
                color: lane.color,
                activity: lane.activity,
                dashed: idle,
                name: lane.name.clone(),
                branch: lane.branch.clone(),
                agent: lane.agent.clone(),
                space: lane.space_index,
                thread: lane.thread_index,
            });
        }
        let caption = SharedString::from(i18n::t!("fleet.graph_tree_caption", "count" => count));
        GraphScene {
            branches,
            main_y: root.1,
            trunk_start_x: root.0,
            trunk_split_x: split.0,
            trunk_end_x: split.1,
            base_commits: Vec::new(),
            head_label: (0.0, 0.0),
            root: Some(root),
            caption: Some(caption),
            labels_below: true,
        }
    }

    /// 系譜グラフ（M14 #4）。ヘッダのスイッチャーで扇形/ツリー/カード/ハブを切替・⌄ で畳む。
    /// 版面ごとに `render_graph_scene`（扇形/ツリー）・`render_graph_cards`・`render_graph_hub` へ委譲する。
    fn render_lineage_graph(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let header = self.render_graph_header(cx);
        if self.chrome.graph_collapsed {
            return div()
                .flex_none()
                .border_b_1()
                .border_color(theme.border)
                .child(header)
                .into_any_element();
        }
        let lanes = self.fleet_lanes(cx);
        let body = match self.chrome.graph_view {
            GraphView::Card => self.render_graph_cards(&lanes, cx),
            GraphView::Hub => self.render_graph_hub(&lanes, cx),
            GraphView::Tree => self.render_graph_scene(self.tree_scene(&lanes), cx),
            GraphView::Fan => self.render_graph_scene(self.radial_scene(&lanes), cx),
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme.border)
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// 系譜グラフのノード/カードをクリックした時: その space へ切り替え、スレッドを focus する
    /// （mock の focus-follows-agent）。カード・扇形/リバー/ツリーの先端が共通で呼ぶ。
    fn focus_fleet_agent(&mut self, space: usize, thread: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_project(space, window, cx);
        if let Some(session) = self.project_sessions.sessions.get(space) {
            let panel = session.agent_panel.clone();
            panel.update(cx, |panel, cx| panel.focus_thread(thread, cx));
        }
        cx.notify();
    }

    /// 版面（扇形/ツリー）を実描画する。canvas に幹・枝の背骨・合流（ネイティブ stroke）を描き、
    /// その上へ overlay で本流コミット・先端ノード（`activity_dot`）・プロバイダ章・ラベルを重ねる。
    /// canvas と overlay は同じ body 矩形に対する割合座標を使うので位置が一致する。先端はクリックで focus。
    fn render_graph_scene(&self, scene: GraphScene, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let count = scene.branches.len();
        // 扇形/リバーはレーン数で伸縮・ツリーは固定。上下に余白を持たせて詰まりを防ぐ。
        let body_h = if scene.root.is_some() {
            300.0
        } else {
            (170.0 + count as f32 * 42.0).clamp(230.0, 380.0)
        };

        // canvas に渡す owned 幾何（SharedString を含まない Spine に絞る）。
        let spines: Vec<Spine> = scene
            .branches
            .iter()
            .map(|branch| Spine {
                split: branch.split,
                control1: branch.control1,
                control2: branch.control2,
                elbow: branch.elbow,
                tip: branch.tip,
                merge: branch.merge,
                color: branch.color,
                dashed: branch.dashed,
            })
            .collect();
        let trunk_color = theme.fg2;
        let main_y = scene.main_y;
        let trunk_start_x = scene.trunk_start_x;
        let trunk_split_x = scene.trunk_split_x;
        let trunk_end_x = scene.trunk_end_x;
        let root = scene.root;

        let graph_canvas = canvas(
            move |_bounds, _window, _cx| {},
            move |bounds, _prepaint, window, _cx| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                let origin_x = f32::from(bounds.origin.x);
                let origin_y = f32::from(bounds.origin.y);
                let at = |xf: f32, yf: f32| point(px(origin_x + xf * width), px(origin_y + yf * height));
                // ── 本流 ──
                if let Some(root_pt) = root {
                    // ツリー: root → split の短い縦幹（全枝が split を共有）。
                    if let Some(first) = spines.first() {
                        let mut trunk = PathBuilder::stroke(px(2.5));
                        trunk.move_to(at(root_pt.0, root_pt.1));
                        trunk.line_to(at(first.split.0, first.split.1));
                        if let Ok(path) = trunk.build() {
                            window.paint_path(path, trunk_color);
                        }
                    }
                } else {
                    // 扇形: 実線 [start, split] ＋ 破線 [split, end]（統合待ちの尾）。
                    let mut solid = PathBuilder::stroke(px(2.5));
                    solid.move_to(at(trunk_start_x, main_y));
                    solid.line_to(at(trunk_split_x, main_y));
                    if let Ok(path) = solid.build() {
                        window.paint_path(path, trunk_color);
                    }
                    let mut dashed = PathBuilder::stroke(px(2.0)).dash_array(&[px(2.0), px(6.0)]);
                    dashed.move_to(at(trunk_split_x, main_y));
                    dashed.line_to(at(trunk_end_x, main_y));
                    if let Ok(path) = dashed.build() {
                        window.paint_path(path, trunk_color.alpha(0.75));
                    }
                }
                // ── 各枝の背骨（split → cubic → elbow → 水平 → tip）──
                for spine in &spines {
                    let mut builder = PathBuilder::stroke(px(2.5));
                    if spine.dashed {
                        builder = builder.dash_array(&[px(2.0), px(6.0)]);
                    }
                    builder.move_to(at(spine.split.0, spine.split.1));
                    builder.cubic_bezier_to(
                        at(spine.elbow.0, spine.elbow.1),
                        at(spine.control1.0, spine.control1.1),
                        at(spine.control2.0, spine.control2.1),
                    );
                    if spine.tip != spine.elbow {
                        builder.line_to(at(spine.tip.0, spine.tip.1));
                    }
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, spine.color);
                    }
                    // 完了枝 → 本流への合流カーブ（扇形の Done のみ・先端から右へ払って main へ戻る）。
                    if let Some(merge) = spine.merge {
                        let mut rejoin = PathBuilder::stroke(px(2.5));
                        rejoin.move_to(at(spine.tip.0, spine.tip.1));
                        rejoin.cubic_bezier_to(
                            at(merge.0, merge.1),
                            at(spine.tip.0 + 0.12, spine.tip.1),
                            at(merge.0 - 0.06, merge.1),
                        );
                        if let Ok(path) = rejoin.build() {
                            window.paint_path(path, spine.color);
                        }
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let mut body = div()
            .relative()
            .flex_none()
            .h(px(body_h))
            .overflow_hidden()
            .child(graph_canvas);

        // 本流コミット（扇形のみ・中空グレー）。
        for (xf, yf) in &scene.base_commits {
            body = body.child(
                dot_overlay(*xf, *yf, 9.0, body_h)
                    .bg(theme.bg1)
                    .border_2()
                    .border_color(theme.fg2),
            );
        }

        // main / HEAD ラベル（扇形/リバー）。
        if scene.root.is_none() {
            body = body.child(
                div()
                    .absolute()
                    .left(relative(scene.head_label.0))
                    .top(px(scene.head_label.1 * body_h - 14.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg1)
                            .child(SharedString::from(i18n::t!("fleet.graph_main"))),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("fleet.graph_main_sub"))),
                    ),
            );
        }

        // ツリーの root ノード＋キャプション。
        if let Some(root_pt) = scene.root {
            body = body.child(
                dot_overlay(root_pt.0, root_pt.1, 13.0, body_h)
                    .bg(theme.bg1)
                    .border_2()
                    .border_color(theme.fg2),
            );
            if let Some(caption) = scene.caption.clone() {
                body = body.child(
                    div()
                        .absolute()
                        .left(relative(root_pt.0))
                        .ml(px(-130.))
                        .top(px(root_pt.1 * body_h - 30.0))
                        .w(px(260.))
                        .flex()
                        .justify_center()
                        .child(div().text_size(px(10.)).text_color(theme.fg2).child(caption)),
                );
            }
        }

        // 各枝の overlay: コミット bead・先端ノード（色＝帰属・形＝状態）・プロバイダ章・ラベル。
        for (index, branch) in scene.branches.iter().enumerate() {
            let (space, thread) = (branch.space, branch.thread);
            let bead_d = 9.0;
            for (bxf, byf) in &branch.beads {
                body = body.child(
                    dot_overlay(*bxf, *byf, bead_d, body_h)
                        .bg(theme.bg1)
                        .border_2()
                        .border_color(branch.color),
                );
            }

            let tip_d = 15.0;
            let outer = tip_d + 6.0; // activity_dot は直径+6 の枠を持つので中心合わせに使う
            body = body.child(
                div()
                    .id(("graph-tip-hit", index))
                    .absolute()
                    .left(relative(branch.tip.0))
                    .ml(px(-outer / 2.0))
                    .top(px(branch.tip.1 * body_h - outer / 2.0))
                    .cursor_pointer()
                    .child(agent_panel::activity_dot(
                        ("graph-tip", index),
                        tip_d,
                        branch.color,
                        branch.activity,
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.focus_fleet_agent(space, thread, window, cx)),
                    ),
            );

            let sub = match &branch.branch {
                Some(name) => {
                    SharedString::from(format!("⎇ {}  ·  {}", name, activity_label(branch.activity)))
                }
                None => activity_label(branch.activity),
            };
            if scene.labels_below {
                // ツリー: ノードの下に中央寄せ（[章 名前] ／ サブ）。クリックで focus。
                body = body.child(
                    div()
                        .id(("graph-tree-label-hit", index))
                        .absolute()
                        .left(relative(branch.tip.0))
                        .ml(px(-90.))
                        .top(px(branch.tip.1 * body_h + outer / 2.0 + 4.0))
                        .w(px(180.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(2.))
                        .cursor_pointer()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .child(agent_panel::agent_badge(branch.agent.as_ref(), 14.0))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.fg0)
                                        .whitespace_nowrap()
                                        .child(branch.name.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(theme.fg2)
                                .whitespace_nowrap()
                                .child(sub),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| this.focus_fleet_agent(space, thread, window, cx)),
                        ),
                );
            } else {
                // 扇形/リバー: 先端の右に（章 ＋ 名前/サブの2行）。クリックで focus。
                body = body.child(
                    div()
                        .id(("graph-label-hit", index))
                        .absolute()
                        .left(relative(branch.tip.0))
                        .ml(px(tip_d / 2.0 + 8.0))
                        .top(px(branch.tip.1 * body_h - 15.0))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .cursor_pointer()
                        .child(agent_panel::agent_badge(branch.agent.as_ref(), 15.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.fg0)
                                        .whitespace_nowrap()
                                        .child(branch.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(9.5))
                                        .text_color(theme.fg2)
                                        .whitespace_nowrap()
                                        .child(sub),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| this.focus_fleet_agent(space, thread, window, cx)),
                        ),
                );
            }
        }

        body.into_any_element()
    }

    /// ハブ表示（GraphRAG／ハブ&スポーク風）。中央にリポジトリのハブ、周囲に各エージェントを楕円状へ配置し、
    /// スレッド色のスポークで結ぶ（idle は破線）。ノード/ラベルはクリックで focus。中央ノードは canvas より
    /// 後（＝上）に描くので、スポークの内側はハブの裏に隠れ、ハブの縁から放射するように見える。
    fn render_graph_hub(&self, lanes: &[FleetLane], cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let body_h = 360.0_f32;
        let count = lanes.len().max(1);
        let center = (0.5_f32, 0.5_f32);
        // パネルが横長なので楕円（横 rx・縦 ry。ともに body 割合）で配置する。横に伸びすぎないよう抑える。
        let (rx, ry) = (0.19_f32, 0.38_f32);
        let repo = self
            .active_worktree()
            .and_then(|worktree| worktree.root().file_name().map(|name| name.to_string_lossy().to_string()))
            .unwrap_or_else(|| i18n::t!("fleet.graph_main"));

        // 各エージェントの配置（真上から時計回り）。canvas と overlay で共有する owned 座標。
        let nodes: Vec<(f32, f32)> = (0..count)
            .map(|index| {
                let theta =
                    -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * index as f32 / count as f32;
                (center.0 + rx * theta.cos(), center.1 + ry * theta.sin())
            })
            .collect();
        let spokes: Vec<((f32, f32), Hsla, bool)> = lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| {
                (nodes[index], lane.color, matches!(lane.activity, agent_panel::ThreadActivity::Idle))
            })
            .collect();

        let hub_center = center;
        let edges = canvas(
            move |_bounds, _window, _cx| {},
            move |bounds, _prepaint, window, _cx| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                let origin_x = f32::from(bounds.origin.x);
                let origin_y = f32::from(bounds.origin.y);
                let at = |xf: f32, yf: f32| point(px(origin_x + xf * width), px(origin_y + yf * height));
                for (pos, color, dashed) in &spokes {
                    let mut spoke = PathBuilder::stroke(px(2.0));
                    if *dashed {
                        spoke = spoke.dash_array(&[px(2.0), px(6.0)]);
                    }
                    spoke.move_to(at(hub_center.0, hub_center.1));
                    spoke.line_to(at(pos.0, pos.1));
                    if let Ok(path) = spoke.build() {
                        window.paint_path(path, *color);
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let mut body = div()
            .relative()
            .flex_none()
            .h(px(body_h))
            .overflow_hidden()
            .child(edges);

        // 各エージェント: 先端ノード（クリック可）＋ 章/名前/状態ラベル（中心の外側へ寄せる）。
        for (index, lane) in lanes.iter().enumerate() {
            let (nx, ny) = nodes[index];
            let (space, thread) = (lane.space_index, lane.thread_index);
            let tip_d = 16.0_f32;
            let outer = tip_d + 6.0;
            body = body.child(
                div()
                    .id(("hub-node", index))
                    .absolute()
                    .left(relative(nx))
                    .ml(px(-outer / 2.0))
                    .top(px(ny * body_h - outer / 2.0))
                    .cursor_pointer()
                    .child(agent_panel::activity_dot(("hub-dot", index), tip_d, lane.color, lane.activity))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.focus_fleet_agent(space, thread, window, cx)),
                    ),
            );

            let sub = match &lane.branch {
                Some(name) => {
                    SharedString::from(format!("⎇ {}  ·  {}", name, activity_label(lane.activity)))
                }
                None => activity_label(lane.activity),
            };
            let name = div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg0)
                .whitespace_nowrap()
                .child(lane.name.clone());
            let sub_line =
                div().text_size(px(9.5)).text_color(theme.fg2).whitespace_nowrap().child(sub);
            // 中心より右のノードはラベルを右へ・左のノードは左へ（中央ハブと重ならないように）。
            let label = if nx >= center.0 {
                div()
                    .id(("hub-label", index))
                    .absolute()
                    .left(relative(nx))
                    .ml(px(tip_d / 2.0 + 8.0))
                    .top(px(ny * body_h - 15.0))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .cursor_pointer()
                    .child(agent_panel::agent_badge(lane.agent.as_ref(), 14.0))
                    .child(div().flex().flex_col().child(name).child(sub_line))
            } else {
                div()
                    .id(("hub-label", index))
                    .absolute()
                    .left(relative(nx))
                    .ml(px(-(168.0 + tip_d / 2.0 + 8.0)))
                    .w(px(168.))
                    .top(px(ny * body_h - 15.0))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(7.))
                    .cursor_pointer()
                    .child(div().flex().flex_col().items_end().child(name).child(sub_line))
                    .child(agent_panel::agent_badge(lane.agent.as_ref(), 14.0))
            };
            body = body.child(label.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.focus_fleet_agent(space, thread, window, cx)),
            ));
        }

        // 中央ハブ（リポジトリ）。スポークの上に重ねるため最後に child する。枠はプロジェクト色。
        body = body.child(
            div()
                .absolute()
                .left(relative(center.0))
                .ml(px(-66.))
                .top(px(center.1 * body_h - 28.0))
                .w(px(132.))
                .h(px(56.))
                .flex()
                .flex_col()
                .justify_center()
                .items_center()
                .gap(px(2.))
                .rounded(px(12.))
                .bg(theme.bg2)
                .border_1()
                .border_color(accent)
                .child(
                    div()
                        .max_w(px(116.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.fg0)
                        .child(SharedString::from(format!("⎇ {repo}"))),
                )
                .child(
                    div()
                        .text_size(px(9.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("fleet.graph_hub_agents", "count" => count))),
                ),
        );

        body.into_any_element()
    }

    /// カード表示（Cursor 風）。左の base ノード → スレッド色の曲線 → 読めるエージェントカード。
    /// 完了カードは右の HEAD ノードへ合流する。曲線は canvas（実ピクセル幅を使ってカードの端に接続）。
    fn render_graph_cards(&self, lanes: &[FleetLane], cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let body_h = 300.0_f32;
        let count = lanes.len().max(1);
        let card_w = 232.0_f32;
        let base_w = 118.0_f32;
        let head_w = 96.0_f32;
        let node_h = 46.0_f32;
        let base_left = 0.015_f32;
        let card_left = 0.30_f32;
        let head_left = 0.86_f32;
        let (top_pad, gap) = (12.0_f32, 12.0_f32);
        let card_h = (((body_h - 2.0 * top_pad) - (count as f32 - 1.0) * gap) / count as f32).clamp(46.0, 76.0);
        let any_done = lanes.iter().any(|lane| lane.integrated);

        // 各カードの矩形（top）と中心 yf（曲線の接続点）を先に確定させる。
        let tops: Vec<f32> = (0..count).map(|i| top_pad + i as f32 * (card_h + gap)).collect();
        // canvas に渡す owned な辺（center_yf, 色, 破線か, 完了か）。
        let edges: Vec<(f32, Hsla, bool, bool)> = lanes
            .iter()
            .enumerate()
            .map(|(i, lane)| {
                (
                    (tops[i] + card_h / 2.0) / body_h,
                    lane.color,
                    matches!(lane.activity, agent_panel::ThreadActivity::Idle),
                    lane.integrated,
                )
            })
            .collect();

        let edge_canvas = canvas(
            move |_bounds, _window, _cx| {},
            move |bounds, _prepaint, window, _cx| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                let origin_x = f32::from(bounds.origin.x);
                let origin_y = f32::from(bounds.origin.y);
                let at = |xf: f32, yf: f32| point(px(origin_x + xf * width), px(origin_y + yf * height));
                let base_right = base_left + base_w / width;
                let card_right = card_left + card_w / width;
                for (center_yf, color, dashed, done) in &edges {
                    let mut builder = PathBuilder::stroke(px(2.5));
                    if *dashed {
                        builder = builder.dash_array(&[px(3.0), px(5.0)]);
                    }
                    let mid = (base_right + card_left) / 2.0;
                    builder.move_to(at(base_right, 0.5));
                    builder.cubic_bezier_to(at(card_left, *center_yf), at(mid, 0.5), at(mid, *center_yf));
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, *color);
                    }
                    if *done {
                        let mut merge = PathBuilder::stroke(px(2.5));
                        let mid2 = (card_right + head_left) / 2.0;
                        merge.move_to(at(card_right, *center_yf));
                        merge.cubic_bezier_to(at(head_left, 0.5), at(mid2, *center_yf), at(mid2, 0.5));
                        if let Ok(path) = merge.build() {
                            window.paint_path(path, *color);
                        }
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let mut body = div()
            .relative()
            .flex_none()
            .h(px(body_h))
            .overflow_hidden()
            .child(edge_canvas);

        // base ノード（左・分岐元）。
        body = body.child(
            div()
                .absolute()
                .left(relative(base_left))
                .top(px(body_h / 2.0 - node_h / 2.0))
                .w(px(base_w))
                .h(px(node_h))
                .flex()
                .flex_col()
                .justify_center()
                .px(px(10.))
                .rounded(px(8.))
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.fg0)
                        .child(SharedString::from(format!("⎇ {}", i18n::t!("fleet.graph_main")))),
                )
                .child(
                    div()
                        .text_size(px(9.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("fleet.graph_card_base"))),
                ),
        );

        // HEAD ノード（右・完了がある時だけ・合流先）。
        if any_done {
            body = body.child(
                div()
                    .absolute()
                    .left(relative(head_left))
                    .top(px(body_h / 2.0 - node_h / 2.0))
                    .w(px(head_w))
                    .h(px(node_h))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .px(px(10.))
                    .rounded(px(8.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from("HEAD")),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("fleet.graph_card_head"))),
                    ),
            );
        }

        // エージェントカード（左バー＝帰属色・章＋名前＋状態ドット・サブ＋トークン）。
        for (index, lane) in lanes.iter().enumerate() {
            let color = lane.color;
            let tokens = SharedString::from(format!(
                "{}/{}",
                agent_panel::human_tokens(lane.tokens_used),
                agent_panel::human_tokens(lane.tokens_max)
            ));
            let sub = match &lane.branch {
                Some(name) => {
                    SharedString::from(format!("⎇ {}  ·  {}", name, activity_label(lane.activity)))
                }
                None => activity_label(lane.activity),
            };
            let space = lane.space_index;
            let thread = lane.thread_index;
            body = body.child(
                div()
                    .id(("graph-card", index))
                    .absolute()
                    .left(relative(card_left))
                    .top(px(tops[index]))
                    .w(px(card_w))
                    .h(px(card_h))
                    .flex()
                    .overflow_hidden()
                    .rounded(px(9.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .hover(|style| style.border_color(color))
                    .child(div().w(px(3.)).h_full().flex_none().bg(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(px(3.))
                            .px(px(10.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .child(agent_panel::agent_badge(lane.agent.as_ref(), 15.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.fg0)
                                            .child(lane.name.clone()),
                                    )
                                    .child(agent_panel::activity_dot(
                                        ("graph-card-dot", index),
                                        10.0,
                                        color,
                                        lane.activity,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(10.))
                                            .text_color(theme.fg2)
                                            .child(sub),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(px(10.))
                                            .text_color(theme.fg2)
                                            .child(tokens),
                                    ),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.focus_fleet_agent(space, thread, window, cx)),
                    ),
            );
        }

        body.into_any_element()
    }

    // ── N 分割グリッド（mock の `.acell`・M14 #3） ──

    /// グリッド本体。**＋ で Agent/Terminal を追加・× で閉じる**（上限 8）。フル画面を使うよう、
    /// 行×列の flex で各セルを `flex_1`（幅も高さも均等に伸びる）。列数はセル数で自動。
    fn render_fleet_grid(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let lanes = self.fleet_lanes(cx);
        let cells = self.chrome.fleet_cells.clone();
        let show_add = cells.len() < 8;
        // ＋ が丸ごと 1 セルを占めるとデッドスペースになる（2026-07-24 ユーザー指摘）。
        // セルが 1 つでもあれば ＋ は下部のスリムバーにして、セルに面積を返す。
        let add_as_cell = show_add && cells.is_empty();
        let total = cells.len() + if add_as_cell { 1 } else { 0 };
        let cols = match total {
            0 | 1 => 1,
            2 => 2,
            3 | 4 => 2,
            5 | 6 => 3,
            _ => 4,
        }
        .max(1);
        // items: 既存セル（index, pane）+ 末尾に ＋ タイル。
        let mut items: Vec<Option<(usize, FleetPane)>> = cells
            .iter()
            .enumerate()
            .map(|(index, pane)| Some((index, pane.clone())))
            .collect();
        if add_as_cell {
            items.push(None);
        }
        let mut grid = div()
            .id("fleet-grid")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.));
        for row in items.chunks(cols) {
            let mut row_element = div().flex_1().min_h_0().flex().gap(px(8.));
            for item in row {
                row_element = row_element.child(match item {
                    Some((index, pane)) => self.render_fleet_cell(*index, pane.clone(), &lanes, false, cx),
                    None => self.render_fleet_add_tile(cx),
                });
            }
            grid = grid.child(row_element);
        }
        // セルがある時の ＋ = 下部スリムバー（1 行 30px・セルの面積を奪わない）。
        if show_add && !add_as_cell {
            let free = 8usize.saturating_sub(self.chrome.fleet_cells.len());
            grid = grid.child(
                div()
                    .id("fleet-add-bar")
                    .flex_none()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .rounded(px(7.))
                    .border_1()
                    .border_dashed()
                    .border_color(self.theme.border)
                    .text_size(px(11.))
                    .text_color(self.theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.border_color(self.theme.fg2))
                    .child(SharedString::from(i18n::t!("fleet.add_agent_simple")))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(self.theme.fg2.alpha(0.7))
                            .child(SharedString::from(format!("{free}/8"))),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            cx.stop_propagation();
                            this.add_fleet_agent_default(cx);
                        }),
                    ),
            );
        }
        grid.into_any_element()
    }

    /// TaskSpace の surface。Task は通常 View と同じ `Entity<AgentPanel>` をそのまま埋め込むため、
    /// markdown / tool call / permission / composer / thread tabs / ACP streaming の全機能が同一実装になる。
    fn render_fleet_cell(
        &self,
        index: usize,
        pane: FleetPane,
        _lanes: &[FleetLane],
        maximized: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let (space, surface_name) = match &pane {
            FleetPane::Task { space } => (space, "Agent"),
            FleetPane::Terminal { space } => (space, "Terminal"),
            FleetPane::Editor { space } => (space, "Editor"),
            FleetPane::Diff { space } => (space, "Diff"),
            FleetPane::Tests { space } => (space, "Tests"),
        };
        let session_index = self.session_index_for_space(space);
        let slot = session_index.and_then(|index| self.project_sessions.projects.get(index));
        let color = slot.map(|slot| slot.color).unwrap_or(theme.fg2);
        let name = slot
            .map(|slot| slot.task_space.title.clone())
            .unwrap_or_else(|| SharedString::from("Closed Task"));
        let branch = slot.and_then(|slot| {
            slot.branch
                .clone()
                .or_else(|| slot.worktree_branch.clone())
                .map(SharedString::from)
        });
        let phase = slot
            .map(|slot| SharedString::from(slot.task_space.phase.as_str()))
            .unwrap_or_else(|| SharedString::from("closed"));
        let task_action = match (&pane, slot.map(|slot| slot.task_space.phase)) {
            (FleetPane::Task { space }, Some(TaskPhase::ReviewReady | TaskPhase::ChangesRequested)) => {
                Some((space.clone(), TaskPhase::ReviewReady))
            }
            (FleetPane::Task { space }, Some(TaskPhase::MergeReady)) => {
                Some((space.clone(), TaskPhase::MergeReady))
            }
            _ => None,
        };

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(9.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme.border)
            .child(div().size(px(8.)).rounded_full().bg(color).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(format!("{name} · {surface_name}"))),
            )
            .when_some(branch, |element, branch| {
                element.child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(format!("⎇ {branch}"))),
                )
            })
            .child(div().flex_none().text_size(px(10.)).text_color(theme.fg2).child(phase))
            .when_some(task_action, |element, (space, action)| {
                let label = if action == TaskPhase::MergeReady {
                    "Integrate"
                } else {
                    "Review"
                };
                element.child(
                    div()
                        .id(("fleet-task-action", index))
                        .flex_none()
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(10.))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                        .child(label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                cx.stop_propagation();
                                if action == TaskPhase::MergeReady {
                                    this.integrate_task(space.clone(), cx);
                                } else {
                                    this.review_task_for_merge(space.clone(), cx);
                                }
                            }),
                        ),
                )
            })
            // 拡大 / 復元（mock focus・ユーザー案）。拡大中は系譜グラフが消えてこのセルが大きくなる。
            .child(
                div()
                    .id(("fleet-cell-max", index))
                    .flex_none()
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .child(
                        svg()
                            .path(if maximized { "icons/minimize.svg" } else { "icons/maximize.svg" })
                            .size(px(11.))
                            .text_color(theme.fg2),
                    )
                    .tooltip(Tooltip::text(
                        i18n::t!(if maximized { "fleet.restore" } else { "fleet.maximize" }),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            if maximized {
                                this.restore_fleet_layout(cx);
                            } else {
                                this.maximize_fleet_cell(index, cx);
                            }
                        }),
                    ),
            )
            .child(
                div()
                    .id(("fleet-cell-close", index))
                    .flex_none()
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                    .child("×")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.close_fleet_cell(index, cx);
                        }),
                    ),
            );

        let mut cell = div()
            .id(("fleet-cell", index))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg0)
            // 上線 = TaskSpace の帰属色。Agent thread の色は埋め込んだ panel 自身が描く。
            .child(div().h(px(2.)).flex_none().bg(color))
            .child(header);
        // 遷移スナップショット帯（P1）: Task セルは「今なにを/どう終わったか」+ plan メーターを 1 行で。
        // 代表 = 最重要 activity のスレッド（lane のロールアップと同じ規則）。digest 無しなら帯ごと出さない。
        if matches!(&pane, FleetPane::Task { .. }) {
            let snapshot = session_index.and_then(|session_index| {
                let statuses = self.project_sessions.sessions[session_index]
                    .agent_panel
                    .read(cx)
                    .statuses();
                statuses
                    .into_iter()
                    .max_by_key(|status| status.activity.urgency())
                    .and_then(|status| {
                        status
                            .digest
                            .clone()
                            .map(|digest| (digest, status.plan_done, status.plan_total))
                    })
            });
            if let Some((digest, plan_done, plan_total)) = snapshot {
                cell = cell.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(9.))
                        .py(px(3.))
                        .border_b_1()
                        .border_color(theme.border)
                        .when(plan_total > 0, |element| {
                            element.child(
                                div()
                                    .flex_none()
                                    .text_size(px(9.))
                                    .text_color(theme.fg2)
                                    .child(plan_meter(plan_done, plan_total)),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(10.))
                                .text_color(theme.fg1)
                                .child(digest),
                        ),
                );
            }
        }
        if let Some(session_index) = session_index {
            let agent_surface = matches!(&pane, FleetPane::Task { .. });
            cell = cell.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.switch_project(session_index, window, cx);
                    this.agent_active = agent_surface;
                }),
            );
        }
        let body = session_index
            .map(|session_index| match pane {
                FleetPane::Task { .. } => self.project_sessions.sessions[session_index]
                    .agent_panel
                    .clone()
                    .into_any_element(),
                FleetPane::Terminal { .. } => self.project_sessions.sessions[session_index]
                    .terminal_dock
                    .clone()
                    .into_any_element(),
                FleetPane::Editor { .. } => self.project_sessions.sessions[session_index]
                    .tabs
                    .get(self.project_sessions.sessions[session_index].active_tab)
                    .map(|tab| tab.editor.clone().into_any_element())
                    .unwrap_or_else(|| {
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.fg2)
                            .child("この Task でファイルを開くと Editor surface に表示されます")
                            .into_any_element()
                    }),
                FleetPane::Diff { .. } => self.project_sessions.sessions[session_index]
                    .git_panel
                    .clone()
                    .into_any_element(),
                FleetPane::Tests { .. } => self.project_sessions.sessions[session_index]
                    .tests_dock
                    .clone()
                    .into_any_element(),
            })
            .unwrap_or_else(|| {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.fg2)
                    .child("TaskSpace は閉じられています")
                    .into_any_element()
            });
        cell = cell.child(div().flex_1().min_h_0().overflow_hidden().child(body));
        cell.into_any_element()
    }

    /// ＋ タイル。既定は隔離 `+ Task`。同一 worktree へ書く Agent は明示操作でだけ追加する。
    fn render_fleet_add_tile(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let add_button = |tag: usize, label: SharedString, theme: &Theme| {
            div()
                .id(("fleet-add-type", tag))
                .px(px(12.))
                .py(px(5.))
                .rounded(px(7.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg0)
                .text_size(px(11.5))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg2).text_color(theme.fg0).border_color(theme.fg2))
                .child(label)
        };
        // ＋ は 1 択（2026-07-24 ユーザー指摘で単純化）: エージェントを新しい worktree で並走させる。
        // Terminal/Editor/Diff/Tests セルや履歴復元の機能自体は残っている（パレット/コード経路）が、
        // 入口の顔からは外す — 「＋ = 並走エージェントを増やす」だけにする。
        let buttons = div()
            .flex()
            .justify_center()
            .child(
                add_button(0, SharedString::from(i18n::t!("fleet.add_agent_simple")), &theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            cx.stop_propagation();
                            this.add_fleet_agent_default(cx);
                        }),
                    ),
            );
        div()
            .id("fleet-add")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_dashed()
            .border_color(theme.border)
            .text_color(theme.fg2)
            .hover(|style| style.border_color(theme.fg2))
            .child(
                div()
                    .size(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(24.))
                    .child(SharedString::from("＋")),
            )
            .child(buttons)
            .child(div().text_size(px(10.5)).child(SharedString::from(i18n::t!("fleet.add_hint"))))
            .into_any_element()
    }

    // ── ニュースフィード（mock 下段） ──

    /// ニュース常設（管制 P2・mock `fleet-dashboard.html` 下段の書式）。ソースは task_events の鏡
    /// （`NotificationCenter.news`・起動時 backfill + 遷移時 live 追記）。行 = 時刻 + 帰属チップ
    /// （スレッド/Task 色・coordinator は丸）+ **太字名** + イベント文。新しいものが上。
    fn render_newsfeed(&self, _cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let head = div()
            .flex_none()
            .flex()
            .items_baseline()
            .gap(px(8.))
            .px(px(12.))
            .pt(px(8.))
            .pb(px(4.))
            .child(
                div()
                    .text_size(px(9.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.fg2)
                    .child(i18n::t!("fleet.news")),
            )
            .child(
                div()
                    .text_size(px(9.))
                    .text_color(theme.fg2.alpha(0.7))
                    .child(i18n::t!("fleet.news_hint")),
            );
        let mut list = div()
            .id("fleet-news")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb(px(4.));
        if self.notifications.news.is_empty() {
            list = list.child(
                div()
                    .px(px(12.))
                    .py(px(4.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(i18n::t!("fleet.news_idle")),
            );
        }
        for (index, item) in self.notifications.news.iter().take(30).enumerate() {
            let chip = div()
                .flex_none()
                .size(px(7.))
                .bg(item.color)
                // 帰属=色・種別=形の規律: エージェント/Task は角丸四角・監督（coordinator）は丸。
                .map(|chip| {
                    if item.kind == NewsKind::Coordinator {
                        chip.rounded_full()
                    } else {
                        chip.rounded(px(2.))
                    }
                });
            list = list.child(
                div()
                    .id(("news-row", index))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(2.))
                    .text_size(px(10.5))
                    .text_color(theme.fg1)
                    .child(
                        div()
                            .flex_none()
                            .w(px(52.))
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(agent_panel::relative_time_label(item.at_ms)),
                    )
                    .child(chip)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .flex()
                            .items_baseline()
                            .gap(px(5.))
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.fg0)
                                    .child(item.title.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(item.text.clone()),
                            ),
                    ),
            );
        }
        div()
            .flex_none()
            .h(px(118.))
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.bg0)
            .child(head)
            .child(list)
            .into_any_element()
    }
}
