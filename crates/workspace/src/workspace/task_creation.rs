//! ＋ Task の作成中の行（O20・A03）— worktree を作っている間・準備スクリプトを流している間を Fleet
//! サイドバーに出し、取り消しとやり直しを置く。
//!
//! 今までは「作る」を押してから worktree と準備スクリプトが終わるまで何も出ず、大きいリポジトリや
//! `pnpm install` を流す準備では、押せたのかどうかも分からなかった。作成は背景で 1 本ずつ走らせ
//! （fan-out も順に・同じリポジトリに `git worktree add` を並べると ref の lock でぶつかる）、
//! 段階が進むたびに行を書き換える。
//!
//! **取り消しは「結果を捨てる」**: 走っている git や準備スクリプトを途中で止める口は Host に無い
//! （SSH 先でも同じ扱いにしたい）。順番待ちの物は作らずに外し、作っている物は、その段が終わった所で
//! worktree と（今切った）ブランチを消す。その間は「取り消しています…」と出す。

use super::fleet_view::FanoutTask;
use crate::workspace::*;

/// 作成中の Task 1 本分（サイドバーの 1 行）。
#[derive(Debug, Clone)]
pub(crate) struct TaskCreation {
    pub(crate) id: u64,
    /// どのリポジトリの編隊に出すか（`repository_key`・サイドバーは選んでいる 1 リポジトリだけを出す）。
    pub(crate) repository_key: String,
    /// 行の 1 段目（依頼の 1 行目・fan-out の印つき）。
    pub(crate) title: SharedString,
    pub(crate) stage: CreationStage,
    /// 取り消しが押された（今の段が終わったら結果を捨てる）。
    pub(crate) cancelled: bool,
    /// やり直し用に、作った時の入力をそのまま持つ。
    pub(crate) prompt: String,
    pub(crate) start: project::TaskStart,
    pub(crate) task: FanoutTask,
}

/// 作成中の Task がどの段にいるか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CreationStage {
    /// fan-out の後ろで順番を待っている。
    Waiting,
    /// 統合先を upstream へ早送りしている（1 回の依頼に 1 回）。
    Syncing,
    /// `git worktree add` と `.worktreeinclude` の持ち込み。
    Worktree,
    /// 準備スクリプト（`.necoder/worktree-setup.sh`）。
    Setup,
    /// 作れなかった（理由の 1 行目）。やり直す / 閉じる。
    Failed(SharedString),
}

/// 1 本分の worktree を作った結果（背景から前景へ渡す）。
struct CreatedWorktree {
    target: PathBuf,
    branch: String,
    /// このブランチを今切った（取り消したら消してよい）。既にあったブランチの worktree なら false。
    new_branch: bool,
    /// `.worktreeinclude` の持ち込みの失敗（準備の失敗と同じ扱いで依頼を控える）。
    include_failure: Option<String>,
    /// 準備スクリプトを流す（置いてあって、今回は飛ばさない）。
    run_setup: bool,
}

/// worktree を作り、`.worktreeinclude` を持ち込む（準備スクリプトはまだ流さない）。背景で呼ぶ。
fn create_worktree_for(
    host: &dyn host::Host,
    root: &Path,
    start: &project::TaskStart,
    task: &FanoutTask,
) -> anyhow::Result<CreatedWorktree> {
    let known = project::git_branches_on(host, root);
    let without_setup = project::TaskStart {
        branch: task.branch.clone(),
        base: start.base.clone(),
        skip_setup: true,
    };
    let (target, branch, include_failure) =
        project::create_task_with_on(host, root, &task.slug_source, &without_setup)?;
    let run_setup =
        !start.skip_setup && host.metadata(&project::worktree_setup_script(root)).is_ok();
    Ok(CreatedWorktree {
        new_branch: !known.contains(&branch),
        target,
        branch,
        include_failure,
        run_setup,
    })
}

/// 取り消した Task の worktree と、今切ったブランチを消す。背景で呼ぶ。
fn discard_worktree(
    host: &dyn host::Host,
    root: &Path,
    created: &CreatedWorktree,
) -> anyhow::Result<()> {
    // 持ち込んだファイルや準備の出力は追跡外なので、force でないと git が断る。
    project::remove_worktree_on(host, root, &created.target, true)?;
    if created.new_branch {
        // 起点から切っただけのブランチ（この作成の中でしか使っていない）。
        project::delete_branch_on(host, root, &created.branch, true)?;
    }
    Ok(())
}

/// 持ち込みの失敗と準備の失敗を 1 つの理由にまとめる（Task のカードに出す）。
fn join_failures(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n{second}")),
        (first, second) => first.or(second),
    }
}

impl Workspace {
    /// 選んでいるリポジトリの統合先から Task を切る（＋ Task・fan-out・O20 / O23）。
    /// 統合先はサイドバー・Captain と同じ選び方（メインの作業ツリー・O21）。
    pub(super) fn create_prompted_tasks(
        &mut self,
        prompt: String,
        start: project::TaskStart,
        plan: Vec<FanoutTask>,
        cx: &mut Context<Self>,
    ) {
        let integration_index = self
            .active_slot()
            .map(|slot| slot.repository_key().to_string())
            .and_then(|key| self.integration_slot_for(&key))
            .unwrap_or(self.project_sessions.active);
        self.create_tasks_in(integration_index, prompt, start, plan, cx);
    }

    /// 1 つの依頼から Task を切る（`plan` が 2 本以上なら fan-out・O23）。worktree は**順に**作り、
    /// 1 本ごとに作成中の行を進める。1 本の失敗で残りを止めない（失敗した行にやり直しを出す）。
    /// 2 本以上なら舞台に並べる（Fleet の中なら横に・外ならトーストで案内）。
    fn create_tasks_in(
        &mut self,
        integration_index: usize,
        prompt: String,
        start: project::TaskStart,
        plan: Vec<FanoutTask>,
        cx: &mut Context<Self>,
    ) {
        if plan.is_empty() {
            return;
        }
        let Some(slot) = self.project_sessions.projects.get(integration_index) else {
            return;
        };
        let repository_key = slot.repository_key().to_string();
        let root = slot.worktree.root().to_path_buf();
        let host = slot.worktree.host().clone();
        // 先頭は早送りから、残りは順番待ちで行を出す。
        let jobs: Vec<(u64, FanoutTask)> = plan
            .into_iter()
            .enumerate()
            .map(|(position, task)| {
                let stage = if position == 0 {
                    CreationStage::Syncing
                } else {
                    CreationStage::Waiting
                };
                let id =
                    self.begin_task_creation(&repository_key, &prompt, &start, task.clone(), stage);
                (id, task)
            })
            .collect();
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            // 土台（統合先の checked-out branch）を先に upstream へ早送りして、Task が古い base から
            // 切られるのを防ぐ（Orca の default branch 自動同期を参考・2026-08-30）。オフライン・dirty・
            // diverged では黙って現 HEAD から続行する。
            let sync = {
                let host = host.clone();
                let root = root.clone();
                cx.background_executor()
                    .spawn(async move { project::sync_current_branch_on(host.as_ref(), &root) })
                    .await
            };
            // Err = 作っている間に窓が閉じた（作った worktree は残り、次に開いた時に外部の worktree として出る）。
            if workspace
                .update(cx, |workspace, cx| workspace.report_base_sync(sync, cx))
                .is_err()
            {
                return;
            }
            let fanout = jobs.len() > 1;
            let mut spaces = Vec::new();
            for (id, task) in jobs {
                match workspace.update(cx, |workspace, cx| {
                    workspace.enter_creation_stage(id, CreationStage::Worktree, cx)
                }) {
                    Err(_) => return,
                    Ok(false) => continue, // 取り消された（まだ何も作っていない）
                    Ok(true) => {}
                }
                let created = {
                    let host = host.clone();
                    let root = root.clone();
                    let start = start.clone();
                    let task = task.clone();
                    cx.background_executor()
                        .spawn(
                            async move { create_worktree_for(host.as_ref(), &root, &start, &task) },
                        )
                        .await
                };
                let created = match created {
                    Ok(created) => created,
                    Err(error) => {
                        if workspace
                            .update(cx, |workspace, cx| {
                                workspace.fail_task_creation(id, &error, cx)
                            })
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                };
                // 準備スクリプト。取り消されていたら流さずに捨てる（finish が消す）。
                let mut setup_failure = None;
                if created.run_setup {
                    let Ok(wanted) = workspace.update(cx, |workspace, cx| {
                        workspace.enter_creation_stage(id, CreationStage::Setup, cx)
                    }) else {
                        return;
                    };
                    if wanted {
                        let host = host.clone();
                        let root = root.clone();
                        let target = created.target.clone();
                        let branch = created.branch.clone();
                        setup_failure = cx
                            .background_executor()
                            .spawn(async move {
                                project::run_task_setup_on(host.as_ref(), &root, &target, &branch)
                                    .err()
                                    .map(|error| format!("{error:#}"))
                            })
                            .await;
                    }
                }
                let Ok(space) = workspace.update(cx, |workspace, cx| {
                    workspace.finish_task_creation(
                        id,
                        &host,
                        &root,
                        created,
                        setup_failure,
                        &prompt,
                        &task,
                        cx,
                    )
                }) else {
                    return;
                };
                spaces.extend(space);
            }
            workspace
                .update(cx, |workspace, cx| {
                    if fanout && !spaces.is_empty() {
                        workspace.show_fanout_on_stage(spaces, cx);
                    }
                    workspace.hint_fleet_mode_once(cx);
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    /// 早送りできた時だけ知らせる（最新から切れた安心情報。スキップは無言＝作成を汚さない）。
    fn report_base_sync(&mut self, sync: project::BranchSyncOutcome, cx: &mut Context<Self>) {
        if let project::BranchSyncOutcome::FastForwarded {
            branch: base,
            commits,
        } = sync
        {
            let accent = self.accent();
            self.push_toast(
                SharedString::from(i18n::t!(
                    "fleet.base_synced",
                    "branch" => &base,
                    "count" => commits
                )),
                accent,
                cx,
            );
        }
    }

    /// 作成中の行を足す（id を返す）。
    fn begin_task_creation(
        &mut self,
        repository_key: &str,
        prompt: &str,
        start: &project::TaskStart,
        task: FanoutTask,
        stage: CreationStage,
    ) -> u64 {
        self.chrome.next_task_creation_id += 1;
        let id = self.chrome.next_task_creation_id;
        let first_line = prompt.lines().next().unwrap_or("").trim();
        let name = if !first_line.is_empty() {
            first_line.to_string()
        } else if !task.slug_source.trim().is_empty() {
            task.slug_source.trim().to_string()
        } else {
            i18n::t!("fleet.creating_untitled")
        };
        let title = match &task.title_suffix {
            Some(suffix) => format!("{name} · {suffix}"),
            None => name,
        };
        self.chrome.task_creations.push(TaskCreation {
            id,
            repository_key: repository_key.to_string(),
            title: title.into(),
            stage,
            cancelled: false,
            prompt: prompt.to_string(),
            start: start.clone(),
            task,
        });
        id
    }

    /// 次の段へ進める。行が無い・取り消されていたら false（作らない / 流さない）。
    fn enter_creation_stage(
        &mut self,
        id: u64,
        stage: CreationStage,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(creation) = self
            .chrome
            .task_creations
            .iter_mut()
            .find(|creation| creation.id == id)
        else {
            return false;
        };
        if creation.cancelled {
            return false;
        }
        creation.stage = stage;
        cx.notify();
        true
    }

    /// worktree を作れなかった: 行を「作れませんでした」にしてやり直しを出す（取り消されていたら外す）。
    fn fail_task_creation(&mut self, id: u64, error: &anyhow::Error, cx: &mut Context<Self>) {
        let Some(position) = self
            .chrome
            .task_creations
            .iter()
            .position(|creation| creation.id == id)
        else {
            return;
        };
        if self.chrome.task_creations[position].cancelled {
            self.chrome.task_creations.remove(position);
            cx.notify();
            return;
        }
        let reason = format!("{error:#}");
        let first_line = reason.lines().next().unwrap_or("").trim().to_string();
        self.chrome.task_creations[position].stage = CreationStage::Failed(first_line.into());
        // サイドバーを見ていない時のために、今までどおりトーストでも知らせる（全文）。
        let accent = self.accent();
        self.push_toast(SharedString::from(reason), accent, cx);
        cx.notify();
    }

    /// 作れた worktree を Task にする（取り消されていたら捨てる）。返すのはその TaskSpace。
    #[allow(clippy::too_many_arguments)]
    fn finish_task_creation(
        &mut self,
        id: u64,
        host: &Arc<dyn host::Host>,
        root: &Path,
        created: CreatedWorktree,
        setup_failure: Option<String>,
        prompt: &str,
        task: &FanoutTask,
        cx: &mut Context<Self>,
    ) -> Option<SpaceId> {
        let position = self
            .chrome
            .task_creations
            .iter()
            .position(|creation| creation.id == id);
        let cancelled =
            position.is_none_or(|position| self.chrome.task_creations[position].cancelled);
        if let Some(position) = position {
            self.chrome.task_creations.remove(position);
        }
        cx.notify();
        if cancelled {
            self.discard_created_task(host.clone(), root.to_path_buf(), created, cx);
            return None;
        }
        let failure = join_failures(created.include_failure, setup_failure);
        self.register_created_task(
            host,
            created.target,
            created.branch,
            failure,
            prompt,
            task,
            cx,
        )
    }

    /// 取り消した Task の worktree を背景で消す。消せなければトーストで場所を知らせる
    /// （残った worktree はサイドバーの「外部の worktree」に出る）。
    fn discard_created_task(
        &mut self,
        host: Arc<dyn host::Host>,
        root: PathBuf,
        created: CreatedWorktree,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |workspace, cx| {
            let target = created.target.clone();
            let result = cx
                .background_executor()
                .spawn(async move { discard_worktree(host.as_ref(), &root, &created) })
                .await;
            // Err = 待っている間に窓が閉じた。
            workspace
                .update(cx, |workspace, cx| {
                    if let Err(error) = result {
                        let accent = workspace.accent();
                        workspace.push_toast(
                            SharedString::from(i18n::t!(
                                "fleet.creating_discard_failed",
                                "path" => target.display(),
                                "error" => format!("{error:#}")
                            )),
                            accent,
                            cx,
                        );
                    }
                    workspace.forget_fleet_worktrees();
                    workspace.refresh_fleet_worktrees(cx);
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    /// 行の × : 順番待ち・早送り中なら作らずに外す。作っている最中なら、その段が終わった所で捨てる。
    /// 作れなかった行なら閉じる。
    pub(crate) fn cancel_task_creation(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(position) = self
            .chrome
            .task_creations
            .iter()
            .position(|creation| creation.id == id)
        else {
            return;
        };
        match self.chrome.task_creations[position].stage {
            CreationStage::Waiting | CreationStage::Syncing | CreationStage::Failed(_) => {
                self.chrome.task_creations.remove(position);
            }
            CreationStage::Worktree | CreationStage::Setup => {
                self.chrome.task_creations[position].cancelled = true;
            }
        }
        cx.notify();
    }

    /// 作れなかった行の「やり直す」: 同じ入力でもう一度作る（その行のリポジトリの統合先から）。
    pub(crate) fn retry_task_creation(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(position) = self
            .chrome
            .task_creations
            .iter()
            .position(|creation| creation.id == id)
        else {
            return;
        };
        if !matches!(
            self.chrome.task_creations[position].stage,
            CreationStage::Failed(_)
        ) {
            return;
        }
        let Some(integration_index) =
            self.integration_slot_for(&self.chrome.task_creations[position].repository_key)
        else {
            return;
        };
        let creation = self.chrome.task_creations.remove(position);
        self.create_tasks_in(
            integration_index,
            creation.prompt,
            creation.start,
            vec![creation.task],
            cx,
        );
    }

    /// 選んでいるリポジトリの作成中の行（Task 行の前に出す）。
    pub(super) fn render_task_creations(
        &self,
        repository_key: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let Some(repository_key) = repository_key else {
            return Vec::new();
        };
        let theme = self.theme.clone();
        self.chrome
            .task_creations
            .iter()
            .filter(|creation| creation.repository_key == repository_key)
            .map(|creation| {
                let id = creation.id;
                let failed = matches!(creation.stage, CreationStage::Failed(_));
                let (status, status_color) = match &creation.stage {
                    _ if creation.cancelled => (i18n::t!("fleet.creating_cancelling"), theme.fg2),
                    CreationStage::Waiting => (i18n::t!("fleet.creating_waiting"), theme.fg2),
                    CreationStage::Syncing => (i18n::t!("fleet.creating_sync"), theme.fg2),
                    CreationStage::Worktree => (i18n::t!("fleet.creating_worktree"), theme.fg2),
                    CreationStage::Setup => (i18n::t!("fleet.creating_setup"), theme.fg2),
                    CreationStage::Failed(reason) => (
                        i18n::t!("fleet.creating_failed", "reason" => reason),
                        theme.warn,
                    ),
                };
                let button =
                    |element_id: (&'static str, usize), label: SharedString, tip: String| {
                        div()
                            .id(element_id)
                            .flex_none()
                            .px(px(5.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(10.))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg2))
                            .child(label)
                            .tooltip(Tooltip::text(tip, theme.clone()))
                    };
                div()
                    .id(("fleet-creating-row", id as usize))
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .min_h(px(40.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    // Task の色はまだ無い（レールに開いた時に決まる）ので、左のバーは地の色。
                    .child(
                        div()
                            .w(px(2.5))
                            .h(px(28.))
                            .rounded_full()
                            .bg(theme.border)
                            .flex_none(),
                    )
                    .child(
                        div()
                            .flex_none()
                            .mt(px(3.))
                            .child(agent_panel::activity_dot(
                                ("fleet-creating-dot", id as usize),
                                9.0,
                                theme.fg2,
                                if failed {
                                    agent_panel::ThreadActivity::Idle
                                } else {
                                    agent_panel::ThreadActivity::Working
                                },
                            )),
                    )
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
                                    .text_color(theme.fg1)
                                    .child(creation.title.clone()),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(10.))
                                    .text_color(status_color)
                                    .child(SharedString::from(status)),
                            ),
                    )
                    .when(failed, |row| {
                        row.child(
                            button(
                                ("fleet-creating-retry", id as usize),
                                SharedString::from(i18n::t!("fleet.creating_retry")),
                                i18n::t!("fleet.creating_retry_tip"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.retry_task_creation(id, cx);
                                }),
                            ),
                        )
                    })
                    .when(!creation.cancelled, |row| {
                        let tip = if failed {
                            i18n::t!("fleet.creating_dismiss_tip")
                        } else {
                            i18n::t!("fleet.creating_cancel_tip")
                        };
                        row.child(
                            button(
                                ("fleet-creating-cancel", id as usize),
                                SharedString::from("×"),
                                tip,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.cancel_task_creation(id, cx);
                                }),
                            ),
                        )
                    })
                    .into_any_element()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .output()
            .expect("git を起動できる")
    }

    fn branch_exists(repo: &Path, name: &str) -> bool {
        git(
            repo,
            &["rev-parse", "--verify", "-q", &format!("refs/heads/{name}")],
        )
        .status
        .success()
    }

    /// 一時リポジトリ（main に 1 コミット・最初から正規化した綴り）と設定ファイル。git が無ければ None。
    fn scratch_repository(name: &str) -> Option<(PathBuf, PathBuf)> {
        let base = std::env::temp_dir().join(format!("necoder_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(base.join("repo")).expect("作業フォルダを作れる");
        let base = paths::canonicalize(&base).expect("正規化できる");
        let repo = base.join("repo");
        if !git(&repo, &["init", "-q", "-b", "main"]).status.success() {
            return None;
        }
        std::fs::write(repo.join("a.txt"), "1\n").expect("書ける");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "first"]);
        std::fs::write(
            base.join("settings.json"),
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .expect("設定を書ける");
        Some((base, repo))
    }

    /// 依頼は空で切る 1 本（エージェントは起こさない・名前は slug の元から）。
    fn task(slug_source: &str, branch: Option<&str>) -> FanoutTask {
        FanoutTask {
            slug_source: slug_source.to_string(),
            branch: branch.map(str::to_string),
            agent: None,
            title_suffix: None,
        }
    }

    fn task_roots(workspace: &Workspace) -> Vec<PathBuf> {
        workspace
            .project_sessions
            .projects
            .iter()
            .map(|slot| slot.worktree.root().to_path_buf())
            .collect()
    }

    /// 押した直後から行が出て、worktree ができたら Task に替わる。
    #[gpui::test]
    fn a_new_task_shows_a_row_until_its_worktree_is_ready(cx: &mut gpui::TestAppContext) {
        let Some((base, repo)) = scratch_repository("task_creation_row") else {
            return;
        };
        cx.update(|cx| settings::init(Some(base.join("settings.json")), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(vec![repo.clone()], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.create_prompted_tasks(
                String::new(),
                project::TaskStart::default(),
                vec![task("Fix the parser", None)],
                cx,
            );
            let creation = workspace
                .chrome
                .task_creations
                .first()
                .cloned()
                .expect("行が出る");
            assert_eq!(creation.title.as_ref(), "Fix the parser");
            assert_eq!(creation.stage, CreationStage::Syncing);
            assert_eq!(
                workspace
                    .render_task_creations(Some(&creation.repository_key), cx)
                    .len(),
                1,
                "選んでいるリポジトリの行として描ける"
            );
            assert!(
                workspace
                    .render_task_creations(Some("other"), cx)
                    .is_empty(),
                "他のリポジトリには出さない"
            );
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, _cx| {
            assert!(
                workspace.chrome.task_creations.is_empty(),
                "できたら行は消える"
            );
            assert!(
                task_roots(workspace)
                    .iter()
                    .any(|root| root.ends_with("fix-the-parser")),
                "Task になった"
            );
        });
        std::fs::remove_dir_all(&base).ok();
    }

    /// 作れなかった行は理由つきで残り、直してから「やり直す」で同じ入力のまま作れる。
    #[gpui::test]
    fn a_failed_creation_can_be_retried(cx: &mut gpui::TestAppContext) {
        let Some((base, repo)) = scratch_repository("task_creation_retry") else {
            return;
        };
        // 統合先が使っているブランチは、別の worktree に出せない（git が断る）。
        assert!(git(&repo, &["switch", "-q", "-c", "feature/busy"])
            .status
            .success());
        cx.update(|cx| settings::init(Some(base.join("settings.json")), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(vec![repo.clone()], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.create_prompted_tasks(
                String::new(),
                project::TaskStart::default(),
                vec![task("Busy", Some("feature/busy"))],
                cx,
            );
        });
        cx.run_until_parked();
        let id = workspace.update_in(cx, |workspace, _window, _cx| {
            let creation = workspace
                .chrome
                .task_creations
                .first()
                .cloned()
                .expect("行が残る");
            assert!(
                matches!(creation.stage, CreationStage::Failed(ref reason) if !reason.is_empty()),
                "{:?}",
                creation.stage
            );
            assert!(!task_roots(workspace)
                .iter()
                .any(|root| root.ends_with("feature-busy")));
            creation.id
        });
        // 直す: 統合先を main に戻せば、そのブランチの worktree を出せる。
        assert!(git(&repo, &["switch", "-q", "main"]).status.success());
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.retry_task_creation(id, cx)
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, _cx| {
            assert!(workspace.chrome.task_creations.is_empty());
            assert!(
                task_roots(workspace)
                    .iter()
                    .any(|root| root.ends_with("feature-busy")),
                "同じブランチの Task になった"
            );
        });
        std::fs::remove_dir_all(&base).ok();
    }

    /// まだ何も作っていない行（早送り中・順番待ち）の × は、その場で外して作らない。
    #[gpui::test]
    fn cancelling_before_the_worktree_creates_nothing(cx: &mut gpui::TestAppContext) {
        let Some((base, repo)) = scratch_repository("task_creation_cancel_early") else {
            return;
        };
        cx.update(|cx| settings::init(Some(base.join("settings.json")), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(vec![repo.clone()], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.create_prompted_tasks(
                String::new(),
                project::TaskStart::default(),
                vec![task("Keep", None), task("Never mind", None)],
                cx,
            );
            let stages: Vec<CreationStage> = workspace
                .chrome
                .task_creations
                .iter()
                .map(|creation| creation.stage.clone())
                .collect();
            assert_eq!(stages, [CreationStage::Syncing, CreationStage::Waiting]);
            let waiting = workspace.chrome.task_creations[1].id;
            workspace.cancel_task_creation(waiting, cx);
            assert_eq!(workspace.chrome.task_creations.len(), 1, "すぐ外れる");
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, _cx| {
            assert!(workspace.chrome.task_creations.is_empty());
            let roots = task_roots(workspace);
            assert!(roots.iter().any(|root| root.ends_with("keep")));
            assert!(!roots.iter().any(|root| root.ends_with("never-mind")));
        });
        assert!(
            !base.join("repo-worktrees").join("never-mind").exists(),
            "worktree も作っていない"
        );
        assert!(!branch_exists(&repo, "task/never-mind"));
        std::fs::remove_dir_all(&base).ok();
    }

    /// 作っている最中の × は印だけ付け、その段が終わった所で準備を流さずに worktree と今切った
    /// ブランチを消す（Task にしない）。
    #[gpui::test]
    fn a_task_cancelled_while_it_was_made_is_removed(cx: &mut gpui::TestAppContext) {
        let Some((base, repo)) = scratch_repository("task_creation_cancel_late") else {
            return;
        };
        cx.update(|cx| settings::init(Some(base.join("settings.json")), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(vec![repo.clone()], Theme::dark(), None, cx));
        let host = host::LocalHost::shared();
        let changed_mind = task("Changed my mind", None);
        let created = create_worktree_for(
            host.as_ref(),
            &repo,
            &project::TaskStart::default(),
            &changed_mind,
        )
        .expect("worktree を作れる");
        let target = created.target.clone();
        assert!(target.exists() && created.new_branch);
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            let key = workspace.project_sessions.projects[0]
                .repository_key()
                .to_string();
            let id = workspace.begin_task_creation(
                &key,
                "",
                &project::TaskStart::default(),
                changed_mind.clone(),
                CreationStage::Worktree,
            );
            workspace.cancel_task_creation(id, cx);
            assert!(
                workspace.chrome.task_creations[0].cancelled,
                "作っている最中は印だけ"
            );
            assert!(
                !workspace.enter_creation_stage(id, CreationStage::Setup, cx),
                "準備は流さない"
            );
            let space = workspace.finish_task_creation(
                id,
                &host,
                &repo,
                created,
                None,
                "",
                &changed_mind,
                cx,
            );
            assert!(space.is_none(), "Task にしない");
            assert!(workspace.chrome.task_creations.is_empty());
        });
        cx.run_until_parked();
        assert!(!target.exists(), "worktree を消した");
        assert!(
            !branch_exists(&repo, "task/changed-my-mind"),
            "今切ったブランチも消した"
        );
        workspace.update_in(cx, |workspace, _window, _cx| {
            assert!(!task_roots(workspace).contains(&target));
        });
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn failures_from_both_steps_are_kept() {
        assert_eq!(join_failures(None, None), None);
        assert_eq!(join_failures(Some("a".into()), None), Some("a".into()));
        assert_eq!(join_failures(None, Some("b".into())), Some("b".into()));
        assert_eq!(
            join_failures(Some("a".into()), Some("b".into())),
            Some("a\nb".into())
        );
    }
}
