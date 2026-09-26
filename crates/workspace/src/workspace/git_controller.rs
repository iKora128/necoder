use crate::workspace::*;

impl Workspace {
    pub(crate) fn git_panel_open(&self, cx: &App) -> bool {
        self.git_panel.read(cx).open
    }

    pub(crate) fn git_is_busy(&self, cx: &App) -> bool {
        self.git_panel.read(cx).busy
    }

    pub(crate) fn close_branch_menu(&self, cx: &mut Context<Self>) -> bool {
        self.git_panel.update(cx, |panel, cx| {
            let was_open = panel.branch_menu.take().is_some();
            if was_open {
                cx.notify();
            }
            was_open
        })
    }

    pub(crate) fn toggle_branch_menu(
        &mut self,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.close_branch_menu(cx) {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let git_panel = self.git_panel.clone();
        cx.spawn(async move |workspace, cx| {
            let (current, branches, worktrees) = cx
                .background_executor()
                .spawn(async move {
                    (
                        project::git_current_branch_on(host.as_ref(), &root),
                        project::git_branches_on(host.as_ref(), &root),
                        project::git_worktrees_on(host.as_ref(), &root),
                    )
                })
                .await;
            let _ = workspace.update(cx, |_workspace, cx| {
                git_panel.update(cx, |panel, cx| {
                    panel.branch_menu = Some(BranchMenuState {
                        position,
                        current,
                        branches,
                        worktrees,
                    });
                    cx.notify();
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn hide_branch_menu(&mut self, cx: &mut Context<Self>) {
        if self.close_branch_menu(cx) {
            cx.notify();
        }
    }

    /// ブランチを in-place で切り替える（git switch）→ プロジェクト再読込。dirty で失敗したらログのみ。
    pub(crate) fn switch_branch_to(
        &mut self,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_branch_menu(cx);
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        // checkout は大きいリポジトリで秒単位になりうる → 背景 + busy 表示。
        let git_panel = self.git_panel.clone();
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let branch_for_open = branch.clone(); // 背景クロージャに move される前に控える（開くとき worktree の branch として使う）
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    // git は「他の worktree にチェックアウト済みのブランチ」への switch を拒否する。
                    // その場合は失敗にせず、**その worktree を開く**へ倒す（並行ブランチの正道）。
                    if let Some(existing) = project::git_worktrees_on(host.as_ref(), &root)
                        .into_iter()
                        .find(|worktree| worktree.branch.as_deref() == Some(branch.as_str()))
                    {
                        if existing.path == root {
                            return Ok(None); // 既にこのブランチに居る
                        }
                        return Ok(Some(existing.path));
                    }
                    project::switch_branch_on(host.as_ref(), &root, &branch).map(|_| None)
                })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                match result {
                    Ok(Some(worktree_path)) => {
                        // レールに居れば切替・無ければレールに開く（⌘O の worktree 行と同じ経路）。
                        workspace.open_worktree_target(
                            worktree_path,
                            Some(branch_for_open),
                            window,
                            cx,
                        );
                    }
                    Ok(None) => workspace.reload_active_project(window, cx),
                    Err(error) => workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ブランチを worktree として**このウィンドウのレール**に開く（並行ブランチ×色付きタブ・M10-2）。
    /// 既存 worktree があればそれを、無ければ `<repo親>/<repo名>-<branch>` に作って開く。新窓は右クリック明示。
    pub(crate) fn open_branch_worktree(&mut self, branch: String, cx: &mut Context<Self>) {
        self.close_branch_menu(cx);
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let repo_name = root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string());
        let Some(parent) = root.parent().map(Path::to_path_buf) else {
            self.push_failure_toast(i18n::t!("git.worktree_no_parent").into(), None, cx);
            return;
        };
        // 列挙も `git worktree add`（checkout 相当で重い）も背景で。busy 表示付き。
        let git_panel = self.git_panel.clone();
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        let host = worktree.host().clone();
        let host_for_open = host.clone(); // 開く側（update クロージャ）用。背景 spawn に host が move される前に控える
        let branch_for_open = branch.clone();
        let root_for_open = root.clone();
        cx.spawn(async move |workspace, cx| {
            let target = cx
                .background_executor()
                .spawn(async move {
                    if let Some(existing) = project::git_worktrees_on(host.as_ref(), &root)
                        .into_iter()
                        .find(|worktree| worktree.branch.as_deref() == Some(branch.as_str()))
                    {
                        return Ok(existing.path);
                    }
                    let sanitized = branch.replace('/', "-");
                    let target = parent.join(format!("{repo_name}-{sanitized}"));
                    project::add_worktree_on(host.as_ref(), &root, &target, &branch)?;
                    Ok::<PathBuf, anyhow::Error>(target)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                match target {
                    // 今チェックアウト中のブランチの ⧉ = 解決先が自分自身。黙って終わると
                    // 「押しても何も起こらない」に見える（2026-08-30 ユーザーが実際に踏んだ）ので言葉で返す。
                    Ok(target) if target == root_for_open => workspace.push_toast(
                        SharedString::from(
                            i18n::t!("git.worktree_is_current", "branch" => &branch_for_open),
                        ),
                        workspace.accent(),
                        cx,
                    ),
                    Ok(target) => workspace.open_folder_in_rail(
                        host_for_open,
                        target,
                        Some(branch_for_open),
                        cx,
                    ),
                    Err(error) => workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// worktree のパスをこのウィンドウのレールに開く（⎇ メニューの worktree 行）。
    pub(crate) fn open_worktree_window(
        &mut self,
        path: PathBuf,
        branch: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.close_branch_menu(cx);
        let host = match self.active_worktree() {
            Some(worktree) => worktree.host().clone(),
            None => host::LocalHost::shared(),
        };
        self.open_folder_in_rail(host, path, branch, cx);
    }

    /// ブランチ切替後などにアクティブプロジェクトを再読込（ツリー再構築・開ファイル再読込・git 更新）。
    pub(crate) fn reload_active_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_active_explorer(cx);
        // 開いていたタブ列を（存在するファイルだけ）開き直す。分割は畳む（旧内容を指すため）。
        self.split_editor = None;
        self.open_slot_files(window, cx);
        self.refresh_git_status(cx);
        self.update_agent_destination(cx);
        cx.notify();
    }

    // ── git 操作パネル（M8: ソース管理。commit / stage / push / pull / 新規ブランチ） ──

    /// git 操作パネルをエクスプローラと切り替える（⌃⇧G）。開くと左カラムを占有しフォーカスを取る。
    pub(crate) fn toggle_git_panel(
        &mut self,
        _: &ToggleGitPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_open = self.git_panel_open(cx);
        self.git_panel
            .update(cx, |panel, cx| panel.set_open(!was_open, cx));
        if was_open {
            // 閉じる → エディタがあればフォーカスを戻す。
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
        } else {
            self.chrome.show_left = true;
            self.chrome.show_herd = false;
            self.todo_panel
                .update(cx, |panel, cx| panel.set_open(false, cx));
            self.focus_git_input(window, cx);
            self.refresh_git_status(cx);
        }
        cx.notify();
    }

    /// ソース管理パネルを作る。コミットメッセージ欄は平坦 `EditorView`（IME・⌘V・undo が効く。
    /// Enter は改行・⌘⏎ でコミット）。入力に合わせて欄を伸ばす（composer と同じ auto-grow）。
    pub(crate) fn new_git_panel(
        theme: &Theme,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> Entity<GitPanel> {
        let message = cx.new(|cx| EditorView::plain(theme.clone(), accent, false, cx));
        cx.subscribe(
            &message,
            |_workspace, _editor, event: &ComposerEvent, cx| {
                if matches!(event, ComposerEvent::ContentHeightChanged) {
                    cx.notify();
                }
            },
        )
        .detach();
        cx.new(|cx| GitPanel::new(message, cx))
    }

    /// パネルの入力欄へフォーカス（ブランチ名の入力中ならそちら・無ければコミットメッセージ）。
    pub(crate) fn focus_git_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = {
            let panel = self.git_panel.read(cx);
            panel
                .branch_name
                .as_ref()
                .unwrap_or(&panel.message)
                .read(cx)
                .focus_handle(cx)
        };
        window.focus(&focus, cx);
    }

    /// Esc（入力欄の `editor::Cancel` が親へ流れてきた / パネル自体にフォーカス）。
    /// ブランチ名の入力中なら入力だけ畳み、そうでなければパネルを閉じる。
    pub(crate) fn cancel_git_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_naming = self.git_panel.update(cx, |panel, cx| {
            let was_naming = panel.branch_name.take().is_some();
            if was_naming {
                cx.notify();
            }
            was_naming
        });
        if was_naming {
            self.focus_git_input(window, cx);
            return;
        }
        self.toggle_git_panel(&ToggleGitPanel, window, cx);
    }

    /// ⌘⏎（`agent::SubmitPrompt`・入力欄が親へ流す）: ブランチ名の入力中なら作成、それ以外はコミット。
    pub(crate) fn submit_git_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_panel.read(cx).branch_name.is_some() {
            self.confirm_new_branch(window, cx);
        } else {
            self.git_commit(window, cx);
        }
    }

    /// パネル自体（入力欄の外）にフォーカスがある時のキー。入力欄の中のキーは EditorView が受ける。
    pub(crate) fn on_git_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" {
            self.cancel_git_input(window, cx);
        }
    }

    /// コミット（何も staged でなければ全部 stage してから）。git はフックで長引きうる → 背景 + busy。
    /// 失敗（フックが止めた等）は理由つきでトーストに出す。
    pub(crate) fn git_commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy(cx) {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let message_editor = self.git_panel.read(cx).message.clone();
        let message = message_editor.read(cx).plain_text();
        if message.trim().is_empty() {
            return;
        }
        let git_panel = self.git_panel.clone();
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let changes = project::git_changes_on(host.as_ref(), &root);
                    if changes.is_empty() {
                        anyhow::bail!("{}", i18n::t!("git.no_commit_changes"));
                    }
                    if !changes.iter().any(|change| change.staged.is_some()) {
                        project::stage_all_on(host.as_ref(), &root)?;
                    }
                    project::commit_on(host.as_ref(), &root, &message)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                match result {
                    Ok(()) => message_editor.update(cx, |editor, cx| editor.clear(cx)),
                    Err(error) => workspace.toast_git_failure(
                        i18n::t!("git.commit_failed"),
                        "git commit",
                        &error,
                        cx,
                    ),
                }
                workspace.refresh_git_status(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// git / gh の失敗を知らせる。トーストは見出し + 理由の要点（`hint:` 行を落とした先頭数行）で、
    /// 収まらない出力（フックの出力など）は押すと `<command> の出力` タブで全文を読める。
    pub(crate) fn toast_git_failure(
        &mut self,
        headline: String,
        command: &str,
        error: &anyhow::Error,
        cx: &mut Context<Self>,
    ) {
        eprintln!("{headline}: {error:#}");
        let output = project::failure_output(error);
        let (gist, truncated) = failure_gist(&output);
        let text = if gist.is_empty() {
            headline
        } else {
            format!("{headline}\n{gist}")
        };
        let details = truncated.then(|| {
            (
                SharedString::from(i18n::t!("git.output_title", "command" => command)),
                output,
            )
        });
        self.push_failure_toast(SharedString::from(text), details, cx);
    }

    /// stage/unstage 系を背景で実行し、完了後に git 状態を更新する共通ヘルパ。
    /// 失敗は `headline`（i18n 済みの見出し）と `command` の出力でトーストに出す。
    pub(crate) fn run_git_index_op(
        &mut self,
        headline: String,
        command: &'static str,
        operation: impl FnOnce(Arc<dyn Host>, PathBuf) -> anyhow::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { operation(host, root) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    workspace.toast_git_failure(headline, command, &error, cx);
                }
                workspace.refresh_git_status(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 1 ファイルを stage。
    pub(crate) fn git_stage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.run_git_index_op(
            i18n::t!("git.stage_failed"),
            "git add",
            move |host, root| project::stage_path_on(host.as_ref(), &root, &path),
            cx,
        );
    }

    /// 1 ファイルを unstage。
    pub(crate) fn git_unstage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.run_git_index_op(
            i18n::t!("git.unstage_failed"),
            "git restore --staged",
            move |host, root| project::unstage_path_on(host.as_ref(), &root, &path),
            cx,
        );
    }

    /// 全変更を stage。
    pub(crate) fn git_stage_all(&mut self, cx: &mut Context<Self>) {
        self.run_git_index_op(
            i18n::t!("git.stage_failed"),
            "git add -A",
            |host, root| project::stage_all_on(host.as_ref(), &root),
            cx,
        );
    }

    /// push（背景実行）。UI を固めない。
    pub(crate) fn git_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_git_remote(true, window, cx);
    }

    /// pull（背景実行）。完了後にプロジェクトを再読込（fast-forward でファイルが変わり得る）。
    pub(crate) fn git_pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_git_remote(false, window, cx);
    }

    /// push/pull をバックグラウンドエグゼキュータで走らせ、完了後に git 状態を更新する。
    pub(crate) fn run_git_remote(
        &mut self,
        is_push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy(cx) {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let git_panel = self.git_panel.clone();
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if is_push {
                        project::push_on(host.as_ref(), &root)
                    } else {
                        project::pull_on(host.as_ref(), &root)
                    }
                })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                match result {
                    Ok(()) if !is_push => workspace.reload_active_project(window, cx),
                    Ok(()) => workspace.refresh_git_status(cx),
                    Err(error) => {
                        let (headline, command) = if is_push {
                            (i18n::t!("git.push_failed"), "git push")
                        } else {
                            (i18n::t!("git.pull_failed"), "git pull")
                        };
                        workspace.toast_git_failure(headline, command, &error, cx);
                        workspace.refresh_git_status(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// GitHub PR 操作（`gh`・背景実行）。`create=true` で PR 作成ページ、false で PR/リポジトリを開く。
    /// git と同じ host 上で走るので remote プロジェクトでもそのまま動く（ブラウザは gh に委ねる）。
    pub(crate) fn github_action(
        &mut self,
        create: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy(cx) {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let git_panel = self.git_panel.clone();
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if create {
                        project::create_pr_on(host.as_ref(), &root)
                    } else {
                        project::open_pr_web_on(host.as_ref(), &root)
                    }
                })
                .await;
            let _ = handle.update(cx, |workspace, _window, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                if let Err(error) = result {
                    let (headline, command) = if create {
                        (i18n::t!("git.pr_create_failed"), "gh pr create")
                    } else {
                        (i18n::t!("git.pr_open_failed"), "gh pr view")
                    };
                    workspace.toast_git_failure(headline, command, &error, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// AI でコミットメッセージを生成（Claude Code CLI に diff を渡す・背景実行）。
    /// 成功したら composer の入力欄に流し込む。AI-agent-native の git 体験。
    pub(crate) fn generate_commit_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy(cx) {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let git_panel = self.git_panel.clone();
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::ai_commit_message_on(host.as_ref(), &root) })
                .await;
            let _ = handle.update(cx, |workspace, _window, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                match result {
                    Ok(message) => {
                        let editor = git_panel.read(cx).message.clone();
                        editor.update(cx, |editor, cx| editor.set_plain_text(&message, cx));
                    }
                    Err(error) => workspace.toast_git_failure(
                        i18n::t!("git.ai_message_failed"),
                        "claude -p",
                        &error,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// git パネルの入力行を「新しいブランチ名」モードにする（＋ボタン）。入力欄は IME の正しい
    /// 1 行の `EditorView`（⏎ で作成・Esc で取消）。
    pub(crate) fn start_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let accent = self.accent();
        let editor = cx.new(|cx| EditorView::plain(self.theme.clone(), accent, true, cx));
        cx.subscribe_in(
            &editor,
            window,
            |workspace, _editor, event: &ComposerEvent, window, cx| {
                if matches!(event, ComposerEvent::Submit) {
                    workspace.confirm_new_branch(window, cx);
                }
            },
        )
        .detach();
        self.git_panel.update(cx, |panel, cx| {
            panel.branch_name = Some(editor);
            cx.notify();
        });
        self.focus_git_input(window, cx);
    }

    /// 入力中のブランチ名で作成＆切替 → プロジェクト再読込。
    pub(crate) fn confirm_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .git_panel
            .read(cx)
            .branch_name
            .as_ref()
            .map(|editor| editor.read(cx).plain_text())
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            self.git_panel.update(cx, |panel, cx| {
                panel.branch_name = None;
                cx.notify();
            });
            self.focus_git_input(window, cx);
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::create_branch_on(host.as_ref(), &root, &name) })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                match result {
                    Ok(()) => {
                        workspace.git_panel.update(cx, |panel, cx| {
                            panel.branch_name = None;
                            cx.notify();
                        });
                        workspace.reload_active_project(window, cx);
                        workspace.focus_git_input(window, cx);
                    }
                    Err(error) => workspace.toast_git_failure(
                        i18n::t!("git.branch_create_failed"),
                        "git switch -c",
                        &error,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ⎇ メニューからブランチを削除（未マージは git が `-d` で拒否＝安全側）。
    /// 他 worktree に checkout 中だと git は拒否する → 事前検知して**分かるトースト**で案内する
    /// （旧実装は失敗が eprintln に消えていた）。成否とも push_toast で可視化。
    pub(crate) fn delete_git_branch(&mut self, branch: String, cx: &mut Context<Self>) {
        self.close_branch_menu(cx);
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let branch_for_msg = branch.clone();
        let git_panel = self.git_panel.clone();
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    // git は「他の worktree に checkout 中のブランチ」削除を拒否する → 先に分かる形で止める。
                    if project::git_worktrees_on(host.as_ref(), &root)
                        .into_iter()
                        .any(|worktree| worktree.branch.as_deref() == Some(branch.as_str()))
                    {
                        anyhow::bail!(
                            "{}",
                            i18n::t!("git.branch_used_by_worktree", "branch" => branch.clone())
                        );
                    }
                    project::delete_branch_on(host.as_ref(), &root, &branch, false)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                match result {
                    Ok(()) => {
                        workspace.push_toast(
                            SharedString::from(
                                i18n::t!("git.branch_deleted", "branch" => branch_for_msg),
                            ),
                            workspace.accent(),
                            cx,
                        );
                        workspace.refresh_git_status(cx);
                    }
                    Err(error) => workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── LSP（言語サーバ・M7。拡張子→サーバの登録式） ──

    // アクティブファイルの言語に合った言語サーバを（必要なら）起動する。
    // 別プロジェクト or 別言語に移ったら張り替える。サーバ未登録の拡張子では何もしない
    // （既存接続は温存＝別タブに戻れば診断が残る）。
}

/// 失敗出力の要点（トースト 1 枚に載せる分）。git の `hint:` 行と空行を落として先頭 3 行・240 文字まで。
/// 落とした行・文字があれば true（全文は別のタブで読ませる）。
fn failure_gist(output: &str) -> (String, bool) {
    const MAX_LINES: usize = 3;
    const MAX_CHARS: usize = 240;
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let kept: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| !line.starts_with("hint:"))
        .take(MAX_LINES)
        .collect();
    let mut truncated = kept.len() < lines.len();
    let mut gist = kept.join("\n");
    if gist.chars().count() > MAX_CHARS {
        gist = format!("{}…", gist.chars().take(MAX_CHARS).collect::<String>());
        truncated = true;
    }
    (gist, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_gist_keeps_short_output_whole() {
        let (gist, truncated) = failure_gist("fatal: not a git repository\n");
        assert_eq!(gist, "fatal: not a git repository");
        assert!(!truncated);
    }

    #[test]
    fn failure_gist_drops_hints_and_offers_the_rest() {
        let push =
            "To github.com:owner/repo.git\n ! [rejected]        main -> main (fetch first)\n\
            error: failed to push some refs to 'github.com:owner/repo.git'\n\
            hint: Updates were rejected because the remote contains work that you do not\n\
            hint: have locally.\n";
        let (gist, truncated) = failure_gist(push);
        assert_eq!(gist.lines().count(), 3);
        assert!(gist.contains("[rejected]") && gist.contains("error: failed to push"));
        assert!(!gist.contains("hint:"));
        assert!(truncated, "hint を落としたら全文を読めるようにする");

        // フックの長い出力は先頭だけ（全文は別タブ）。
        let hook: String = (1..=10)
            .map(|line| format!("lint error {line}\n"))
            .collect();
        let (gist, truncated) = failure_gist(&hook);
        assert_eq!(gist, "lint error 1\nlint error 2\nlint error 3");
        assert!(truncated);
    }

    /// ソース管理パネル（受入）: 変更の行を押すと diff タブが開く。コミット欄は ⌘V・打鍵・undo が効き、
    /// ⌘⏎ のコミットが pre-commit フックで止まると、フックの出力つきのトースト（全文つき）が出て
    /// メッセージは残る。
    #[cfg(unix)]
    #[gpui::test]
    fn git_panel_rows_open_diffs_and_hook_failures_reach_a_toast(cx: &mut gpui::TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;
        let root = std::env::temp_dir().join(format!(
            "necoder_git_panel_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("hooks")).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            return; // git 無し環境はスキップ
        }
        for (key, value) in [
            ("user.email", "t@example.com"),
            ("user.name", "tester"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "hooks"),
        ] {
            assert!(git(&["config", key, value]));
        }
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        assert!(git(&["add", "-A"]) && git(&["commit", "-q", "-m", "init"]));
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();
        let hook = repo.join("hooks").join("pre-commit");
        std::fs::write(
            &hook,
            "#!/bin/sh\nfor n in 1 2 3 4 5; do echo \"pre-commit: lint error $n\" >&2; done\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| {
            settings::init(Some(settings_path), None, cx);
            // ⌘V / ⌘Z / ⌘⏎ を実キーで通すため既定 keymap を張る。
            let bindings = keymap_core::load_bindings(keymap_core::DEFAULT_KEYMAP_JSON, cx)
                .expect("既定 keymap がロードできる");
            cx.bind_keys(bindings);
        });
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(vec![repo.clone()], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
                session
                    .terminal_dock
                    .update(cx, |dock, _| dock.use_test_terminals());
            }
            workspace.toggle_git_panel(&ToggleGitPanel, window, cx);
        });
        cx.run_until_parked();

        // 変更の行を押す → HEAD との diff タブ。
        let row = cx
            .debug_bounds("git-unstaged-0")
            .expect("ソース管理パネルに変更の行が描かれている");
        cx.simulate_mouse_down(row.center(), MouseButton::Left, gpui::Modifiers::none());
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            let tab = &workspace.tabs[workspace.active_tab];
            assert_eq!(
                tab.path.file_name().and_then(|name| name.to_str()),
                Some("a.txt ⇄ HEAD")
            );
            let text = tab
                .editor()
                .map(|editor| editor.read(cx).plain_text())
                .unwrap_or_default();
            assert!(text.contains("-one") && text.contains("+two"), "{text}");
        });

        // コミット欄: ⌘V（テスト用クリップボード）・打鍵（IME と同じ入力経路）・⌘Z。
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.focus_git_input(window, cx)
        });
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("fix: 貼り付け".to_string()));
        cx.simulate_keystrokes("cmd-v");
        cx.simulate_input(" と打鍵");
        cx.run_until_parked();
        let message = |workspace: &Workspace, cx: &App| {
            workspace.git_panel.read(cx).message.read(cx).plain_text()
        };
        workspace.update_in(cx, |workspace, _window, cx| {
            assert_eq!(message(workspace, cx), "fix: 貼り付け と打鍵");
        });
        // ⌘Z は打鍵を戻す（戻す単位はバッファの undo の粒度）。貼り付けまでは戻さない。
        cx.simulate_keystrokes("cmd-z");
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            let undone = message(workspace, cx);
            assert!(
                undone.len() < "fix: 貼り付け と打鍵".len() && undone.starts_with("fix: 貼り付け"),
                "⌘Z で打鍵が戻らない: {undone}"
            );
            let editor = workspace.git_panel.read(cx).message.clone();
            editor.update(cx, |editor, cx| editor.set_plain_text("fix: 貼り付け", cx));
        });

        // ⌘⏎ でコミット → pre-commit が止める → フックの出力つきのトースト。メッセージは残る。
        cx.simulate_keystrokes("cmd-enter");
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            let toasts = workspace.toast_snapshot();
            let (text, has_details) = toasts.last().cloned().expect("失敗のトーストが出る");
            assert!(text.contains("pre-commit: lint error 1"), "{text}");
            assert!(has_details, "5 行の出力は要点に収まらないので全文を開ける");
            assert_eq!(
                message(workspace, cx),
                "fix: 貼り付け",
                "失敗したらメッセージを消さない"
            );
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        let count = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&count.stdout).trim(),
            "1",
            "フックが止めたのでコミットは増えない"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failure_gist_clips_long_lines() {
        let (gist, truncated) = failure_gist(&"x".repeat(500));
        assert_eq!(gist.chars().count(), 241, "240 文字 + …");
        assert!(gist.ends_with('…') && truncated);
    }
}
