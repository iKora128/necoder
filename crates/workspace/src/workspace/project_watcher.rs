use crate::workspace::*;

impl Workspace {
    pub(crate) fn start_watcher(&mut self, cx: &mut Context<Self>) {
        let session_index = self.project_sessions.active;
        let session = self.session_mut();
        session._watch = None;
        session._watch_pump = None;
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        // remote も張る（M13）: local=notify / remote=Host 経由の poll。差し替えは project::watch_root 内。
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
        let debug = std::env::var_os("NECODER_WATCH_DEBUG").is_some();
        match project::watch_root(&host, &root, move |paths| {
            if debug {
                eprintln!("watch: raw event {paths:?}");
            }
            let _ = sender.unbounded_send(paths);
        }) {
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
            if let Some(slot) = self.project_sessions.projects.get_mut(session_index) {
                slot.refresh();
            }
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
