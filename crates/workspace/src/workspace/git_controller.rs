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

    pub(crate) fn toggle_branch_menu(&mut self, position: Point<gpui::Pixels>, cx: &mut Context<Self>) {
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
                    panel.branch_menu =
                        Some(BranchMenuState { position, current, branches, worktrees });
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
    pub(crate) fn switch_branch_to(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
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
            eprintln!("worktree の作成先を決められない（root に親が無い）");
            cx.notify();
            return;
        };
        // 列挙も `git worktree add`（checkout 相当で重い）も背景で。busy 表示付き。
        let git_panel = self.git_panel.clone();
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        let host = worktree.host().clone();
        let host_for_open = host.clone(); // 開く側（update クロージャ）用。背景 spawn に host が move される前に控える
        let branch_for_open = branch.clone();
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
                    Ok(target) => {
                        workspace.open_folder_in_rail(host_for_open, target, Some(branch_for_open), cx)
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

    /// worktree のパスをこのウィンドウのレールに開く（⎇ メニューの worktree 行）。
    pub(crate) fn open_worktree_window(&mut self, path: PathBuf, branch: Option<String>, cx: &mut Context<Self>) {
        self.close_branch_menu(cx);
        let host = match self.active_worktree() {
            Some(worktree) => worktree.host().clone(),
            None => host::LocalHost::shared(),
        };
        self.open_folder_in_rail(host, path, branch, cx);
    }

    /// ブランチ切替後などにアクティブプロジェクトを再読込（ツリー再構築・開ファイル再読込・git 更新）。
    pub(crate) fn reload_active_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.projects.get_mut(active) {
            slot.refresh();
        }
        // 開いていたタブ列を（存在するファイルだけ）開き直す。分割は畳む（旧内容を指すため）。
        self.split_editor = None;
        self.open_slot_files(window, cx);
        self.refresh_git_status(cx);
        self.update_agent_destination(cx);
        cx.notify();
    }

    // ── git 操作パネル（M8: ソース管理。commit / stage / push / pull / 新規ブランチ） ──

    /// git 操作パネルをエクスプローラと切り替える（⌃⇧G）。開くと左カラムを占有しフォーカスを取る。
    pub(crate) fn toggle_git_panel(&mut self, _: &ToggleGitPanel, window: &mut Window, cx: &mut Context<Self>) {
        let was_open = self.git_panel_open(cx);
        self.git_panel.update(cx, |panel, cx| panel.set_open(!was_open, cx));
        if was_open {
            // 閉じる → エディタがあればフォーカスを戻す。
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
        } else {
            self.chrome.show_left = true;
            self.chrome.show_herd = false;
            self.todo_panel.update(cx, |panel, cx| panel.set_open(false, cx));
            let focus = self.git_panel.read(cx).focus.clone();
            window.focus(&focus, cx);
            self.refresh_git_status(cx);
        }
        cx.notify();
    }

    /// git パネルのキー入力（コミットメッセージ / ブランチ名を手書きで積む。検索パネルと同流儀）。
    pub(crate) fn on_git_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                // ブランチ名モードなら入力だけ畳む。そうでなければパネルを閉じる。
                let was_naming = self.git_panel.update(cx, |panel, cx| {
                    let was_naming = panel.branch_name.take().is_some();
                    if was_naming {
                        cx.notify();
                    }
                    was_naming
                });
                if was_naming {
                    return;
                }
                self.toggle_git_panel(&ToggleGitPanel, window, cx);
            }
            "enter" => {
                let naming = self.git_panel.read(cx).branch_name.is_some();
                if naming {
                    self.confirm_new_branch(window, cx);
                } else if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
                    self.git_commit(window, cx); // ⌘/⌃⏎ = コミット
                } else {
                    self.git_panel.update(cx, |panel, cx| {
                        panel.message.push('\n'); // 素の Enter は改行
                        cx.notify();
                    });
                }
            }
            "backspace" => {
                self.git_panel.update(cx, |panel, cx| {
                    match &mut panel.branch_name {
                        Some(name) => {
                            name.pop();
                        }
                        None => {
                            panel.message.pop();
                        }
                    }
                    cx.notify();
                });
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty() && !text.chars().any(char::is_control) {
                        self.git_panel.update(cx, |panel, cx| {
                            match &mut panel.branch_name {
                                Some(name) => name.push_str(text),
                                None => panel.message.push_str(text),
                            }
                            cx.notify();
                        });
                    }
                }
            }
        }
    }

    /// staged 変更をコミット。staged が無ければ全変更を stage してからコミット（簡便動線）。
    /// コミット（何も staged でなければ全部 stage してから）。git はフックで長引きうる → 背景 + busy。
    pub(crate) fn git_commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy(cx) {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let message = self.git_panel.read(cx).message.clone();
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
                    Ok(()) => {
                        git_panel.update(cx, |panel, cx| {
                            panel.message.clear();
                            cx.notify();
                        });
                    }
                    Err(error) => eprintln!("コミットに失敗: {error:#}"),
                }
                workspace.refresh_git_status(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// stage/unstage 系を背景で実行し、完了後に git 状態を更新する共通ヘルパ。
    pub(crate) fn run_git_index_op(
        &mut self,
        describe: String,
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
                    eprintln!("{describe}に失敗: {error:#}");
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
            "stage".to_string(),
            move |host, root| project::stage_path_on(host.as_ref(), &root, &path),
            cx,
        );
    }

    /// 1 ファイルを unstage。
    pub(crate) fn git_unstage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.run_git_index_op(
            "unstage".to_string(),
            move |host, root| project::unstage_path_on(host.as_ref(), &root, &path),
            cx,
        );
    }

    /// 全変更を stage。
    pub(crate) fn git_stage_all(&mut self, cx: &mut Context<Self>) {
        self.run_git_index_op(
            "stage".to_string(),
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
    pub(crate) fn run_git_remote(&mut self, is_push: bool, window: &mut Window, cx: &mut Context<Self>) {
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
                        eprintln!("{} に失敗: {error:#}", if is_push { "push" } else { "pull" });
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
    pub(crate) fn github_action(&mut self, create: bool, window: &mut Window, cx: &mut Context<Self>) {
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
            let _ = handle.update(cx, |_workspace, _window, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                if let Err(error) = result {
                    eprintln!("GitHub 操作に失敗: {error:#}");
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
            let _ = handle.update(cx, |_workspace, _window, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                match result {
                    Ok(message) => {
                        git_panel.update(cx, |panel, cx| {
                            panel.message = message;
                            cx.notify();
                        });
                    }
                    Err(error) => eprintln!("コミットメッセージ生成に失敗: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// git パネルの入力行を「新しいブランチ名」モードにする（＋ボタン）。
    pub(crate) fn start_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.git_panel.update(cx, |panel, cx| {
            panel.branch_name = Some(String::new());
            cx.notify();
            panel.focus.clone()
        });
        window.focus(&focus, cx);
    }

    /// 入力中のブランチ名で作成＆切替 → プロジェクト再読込。
    pub(crate) fn confirm_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .git_panel
            .read(cx)
            .branch_name
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            self.git_panel.update(cx, |panel, cx| {
                panel.branch_name = None;
                cx.notify();
            });
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
                    }
                    Err(error) => eprintln!("ブランチ作成に失敗: {error:#}"),
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
