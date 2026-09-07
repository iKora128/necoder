use crate::workspace::*;

/// ファイル監視（notify / remote の watch pump）を張るか。
///
/// gpui のテストスケジューラは決定的で、**別 OS スレッドからタスクを起こすと「非決定的」と判定して
/// テストを落とす**。notify の監視スレッド（Windows は `notify-rs windows loop`）が temp ディレクトリの
/// 変化で pump を起こし、Windows CI の `expanding_a_remote_directory_never_blocks_the_ui_thread` が
/// 落ちた（2026-09-07）。各テストが teardown で watcher を先に落とす回避は競合が残る（drop と
/// 監視スレッドは非同期）ので、テストビルドでは監視自体を張らない。テストは watch イベントに
/// 依存しない（必要なら `handle_watch_events` を直接呼ぶ）。
fn file_watching_enabled() -> bool {
    !cfg!(test)
}

impl Workspace {
    pub(crate) fn start_watcher(&mut self, cx: &mut Context<Self>) {
        let session_index = self.project_sessions.active;
        let session = self.session_mut();
        session._watch = None;
        session._watch_pump = None;
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        if !file_watching_enabled() {
            return;
        }
        // remote も張る（M13）: local=notify / remote=Host 経由の poll。差し替えは project::watch_root 内。
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
        let debug = std::env::var_os("NECODER_WATCH_DEBUG").is_some();
        let on_paths = move |paths: Vec<PathBuf>| {
            if debug {
                eprintln!("watch: raw event {paths:?}");
            }
            let _ = sender.unbounded_send(paths);
        };
        if host.is_remote() {
            // remote の監視開始は daemon への `Watch` request ＝ SSH の往復。UI スレッドで待たない
            // （切替のたびに 1 往復ぶん固まるし、接続が不調なら再接続待ちまで背負う）。
            // pump は下で先に立つので、監視が張れた瞬間からイベントを取りこぼさない。
            let watch_root = root.clone();
            cx.spawn(async move |workspace, cx| {
                let watch = cx
                    .background_executor()
                    .spawn(async move { project::watch_root(&host, &watch_root, on_paths) })
                    .await;
                let _ = workspace.update(cx, |workspace, _cx| match watch {
                    Ok(watch) => {
                        // 張っている間にレールが並び替わっていたら、その watch は捨てる。
                        let still_same = workspace
                            .project_sessions
                            .projects
                            .get(session_index)
                            .is_some_and(|slot| slot.worktree.root() == root);
                        if !still_same {
                            return;
                        }
                        if debug {
                            eprintln!("watch: 監視開始 {}", root.display());
                        }
                        if let Some(session) =
                            workspace.project_sessions.sessions.get_mut(session_index)
                        {
                            session._watch = Some(watch);
                        }
                    }
                    Err(error) => eprintln!("{error:#}"),
                });
            })
            .detach();
        } else {
            match project::watch_root(&host, &root, on_paths) {
                Ok(watch) => {
                    if debug {
                        eprintln!("watch: 監視開始 {}", root.display());
                    }
                    self.session_mut()._watch = Some(watch);
                }
                Err(error) => {
                    eprintln!("{error:#}");
                    return;
                }
            }
        }
        self.session_mut()._watch_pump = Some(cx.spawn(async move |workspace, cx| {
            if debug {
                eprintln!("watch: pump 稼働");
            }
            while let Some(first) = receiver.next().await {
                let mut paths = first;
                // 200ms 合流（cargo build 等の連続イベントを 1 回に畳む）。
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                while let Ok(more) = receiver.try_recv() {
                    paths.extend(more);
                }
                paths.sort();
                paths.dedup();
                if debug {
                    eprintln!("watch: 合流 {} paths", paths.len());
                }
                let updated = workspace.update(cx, |workspace, cx| {
                    workspace.handle_watch_events(session_index, paths, cx)
                });
                if debug {
                    eprintln!(
                        "watch: update {:?}",
                        updated.as_ref().map(|_| "ok").map_err(|e| format!("{e}"))
                    );
                }
                if updated.is_err() {
                    break;
                }
            }
        }));
    }

    /// watch イベント（合流済みパス群）を反映する:
    /// ①開いているバッファの外部変更（自動リロード / dirty なら警告バー）
    /// ②ツリー再構築 ③git 色 + gutter diff の更新。gitignore 対象（target/ 等）はノイズとして落とす。
    pub(crate) fn handle_watch_events(
        &mut self,
        session_index: usize,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.worktree.clone())
        else {
            return;
        };
        let mut tree_changed = false;
        let mut git_changed = false;
        for path in &paths {
            // .git 配下は index/HEAD/refs だけ git 更新の合図に使う（objects 等の湧きは無視）。
            if path
                .components()
                .any(|component| component.as_os_str() == ".git")
            {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                let in_refs = path
                    .components()
                    .any(|component| component.as_os_str() == "refs");
                if in_refs || matches!(name, "index" | "HEAD" | "ORIG_HEAD" | "packed-refs") {
                    git_changed = true;
                }
                continue;
            }
            // 開いているバッファへ配送（gitignore に関係なく）。
            if let Some(tab) = self.project_sessions.sessions[session_index]
                .tabs
                .iter()
                .find(|tab| &tab.path == path)
            {
                match &tab.content {
                    TabContent::Editor { editor, .. } => {
                        let editor = editor.clone();
                        editor.update(cx, |view, cx| view.handle_external_change(cx));
                    }
                    // 画像はディスクの新内容を背景で読み直して差し替える
                    // （スクショの再生成等・エディタの自動リロードと同じ「外は正」方針）。
                    TabContent::Image(view) => {
                        let view = view.clone();
                        let host = worktree.host().clone();
                        let image_path = path.clone();
                        cx.spawn(async move |_workspace, cx| {
                            let content = cx
                                .background_executor()
                                .spawn({
                                    let image_path = image_path.clone();
                                    async move { host.read_file(&image_path) }
                                })
                                .await;
                            match content {
                                Ok(content) => {
                                    // Err = タブが閉じられ view が消えた後に読み終えた（無害）。
                                    let _ = view.update(cx, |view, cx| {
                                        view.set_bytes(&image_path, content.bytes, cx)
                                    });
                                }
                                Err(error) => {
                                    eprintln!("画像を再読み込みできない: {error:#}")
                                }
                            }
                        })
                        .detach();
                    }
                }
                git_changed = true;
                continue;
            }
            if !worktree.is_ignored(path) {
                tree_changed = true;
                git_changed = true;
            }
        }
        if tree_changed {
            // remote は背景で読む（watch はイベントのたびに来る。ここで往復すると、接続が
            // 不調なときにイベント 1 回ごとに固まる）。
            self.refresh_explorer_for(session_index, cx);
        }
        if git_changed {
            self.refresh_git_status_for(session_index, cx);
            let session = &self.project_sessions.sessions[session_index];
            if let Some(editor) = session
                .tabs
                .get(session.active_tab)
                .and_then(|tab| tab.editor().cloned())
            {
                editor.update(cx, |view, cx| view.refresh_diff(cx));
            }
        }
        // Todo ボード: どの書き手（AI/CLI/手編集）が todos.md を変えても板が追従する（M12-10 の心臓部）。
        if paths
            .iter()
            .any(|path| path.ends_with(std::path::Path::new(".necoder/todos.md")))
        {
            self.reload_todo_board_for(session_index, cx);
        }
        if tree_changed || git_changed {
            cx.notify();
        }
    }

    /// settings の実効値（font_size/tab_size/soft_wrap）を全エディタへ配る（live 反映・M10-13）。
    pub(crate) fn apply_editor_settings(&mut self, cx: &mut Context<Self>) {
        let current = settings::get(cx);
        let soft_wrap = current.soft_wrap || std::env::var_os("NECODER_SOFT_WRAP").is_some();
        let (font_size, tab_size) = (current.font_size, current.tab_size);
        let editors: Vec<Entity<EditorView>> = self
            .project_sessions
            .sessions
            .iter()
            .flat_map(|session| {
                session
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.editor().cloned())
                    .chain(session.split_editor.clone())
            })
            .collect();
        for editor in editors {
            editor.update(cx, |view, cx| {
                view.set_typography(font_size, tab_size, cx);
                view.set_soft_wrap(soft_wrap, cx);
                view.set_html_preview_evict_minutes(current.html_preview_evict_minutes, cx);
            });
        }
    }

    // ── プロジェクト色ピッカー（レール右クリック → .necoder/settings.json へ・M12-11） ──

    // 選んだ色をプロジェクトへ適用し `.necoder/settings.json` に保存（再起動後も効く）。
}
