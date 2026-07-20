use crate::workspace::*;

impl Workspace {
    pub(crate) fn open_file_finder(&mut self, _: &FileFinder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let worktree_for_list = worktree.clone();
        let files_task = cx.background_executor().spawn({
            let host = worktree_for_list.host().clone();
            let root = worktree_for_list.root().to_path_buf();
            // 50k: 実測で refilter ~10ms/キー = 1 フレーム内（examples/bench_fuzzy・
            // terminal-stack-2026 §4 の「fzf+rg に負けない」ライン）。列挙は背景なので開く速さに影響しない。
            async move { project::all_files_on(host.as_ref(), &root, 50_000) }
        });
        cx.spawn(async move |_workspace, cx| {
            let files = files_task.await;
            let _ = handle.update(cx, |workspace, window, cx| {
                let items = files
                    .iter()
                    .enumerate()
                    .map(|(id, (_, relative))| PickerItem::new(id, relative.clone()))
                    .collect();
                workspace.overlays.picker_files = files.into_iter().map(|(path, _)| path).collect();
                workspace.open_picker(PickerMode::Files, i18n::t!("finder.files"), items, window, cx);
            });
        })
        .detach();
    }

    /// ⌘⇧P コマンドパレット（M13）: [`CommandRegistry`] を名前+キー併記で並べ、
    /// 確定でアクションを dispatch する（閉じてから = エディタ/Workspace コンテキストで解決）。
    pub(crate) fn open_command_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        let sections = keymap_core::parse(keymap_core::DEFAULT_KEYMAP_JSON).unwrap_or_default();
        let items = COMMAND_REGISTRY
            .entries()
            .iter()
            .enumerate()
            .map(|(id, entry)| {
                let mut item = PickerItem::new(id, i18n::t!(entry.label_key));
                if let Some(keystrokes) =
                    keymap_core::key_for_action(&sections, entry.action_name)
                {
                    item = item.with_detail(keymap_core::pretty_keystroke(&keystrokes));
                }
                item
            })
            .collect();
        self.open_picker(PickerMode::Commands, i18n::t!("palette.placeholder"), items, window, cx);
    }

    /// ⌘O スイッチャー（2 階層・M12-12）: プロジェクト行 + 配下の branch/worktree 行を
    /// 1 リストに並べる（UI-SPEC §7）。まずプロジェクト行だけで即開き、worktree 一覧と
    /// ahead/behind/dirty は背景で集めて後から差し込む（開く速さを守る）。
    pub(crate) fn open_project_switcher(&mut self, _: &ProjectSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_worktree_rows = Vec::new();
        let items = self.build_switcher_items(&[], cx).0;
        self.open_picker(PickerMode::Projects, i18n::t!("finder.projects"), items, window, cx);
        // 背景収集: 各プロジェクトの worktree 一覧 + それぞれの status（git 2 コマンド/worktree）。
        let sources: Vec<(usize, Arc<dyn Host>, PathBuf)> = self
            .project_sessions
            .projects
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                (index, slot.worktree.host().clone(), slot.worktree.root().to_path_buf())
            })
            .collect();
        cx.spawn(async move |workspace, cx| {
            let collected = cx
                .background_executor()
                .spawn(async move {
                    sources
                        .into_iter()
                        .map(|(index, host, root)| {
                            let rows: Vec<(project::GitWorktree, project::WorktreeStatus)> =
                                project::git_worktrees_on(host.as_ref(), &root)
                                    .into_iter()
                                    .map(|worktree| {
                                        let status = project::worktree_status_on(
                                            host.as_ref(),
                                            &worktree.path,
                                        )
                                        .unwrap_or_default();
                                        (worktree, status)
                                    })
                                    .collect();
                            (index, rows)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                // ⌘O がまだ Projects モードで開いている時だけ差し込む。
                if workspace.overlays.picker_mode != PickerMode::Projects {
                    return;
                }
                let Some(picker) = workspace.overlays.picker.clone() else {
                    return;
                };
                let (items, rows) = workspace.build_switcher_items(&collected, cx);
                workspace.picker_worktree_rows = rows;
                picker.update(cx, |picker, cx| picker.set_items(items, cx));
            });
        })
        .detach();
    }

    /// ⌘O の行を組む: プロジェクト行（●色 + 名前 + パス + 実行中ドット）+ 配下の
    /// worktree 行（⎇ branch + ↑↓/dirty + 実行中ドット）。戻りは (items, id-1000 → path 表)。
    pub(crate) fn build_switcher_items(
        &self,
        collected: &[(usize, Vec<(project::GitWorktree, project::WorktreeStatus)>)],
        cx: &App,
    ) -> (Vec<PickerItem>, Vec<PathBuf>) {
        let registry = cx.try_global::<agent_panel::RunningRegistry>();
        let running_dots = |path: &Path| -> Vec<Hsla> {
            registry
                .and_then(|registry| registry.0.get(path))
                .map(|rows| {
                    rows.iter()
                        .filter(|(_, _, running)| *running)
                        .map(|(_, color, _)| *color)
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut items = Vec::new();
        let mut rows: Vec<PathBuf> = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            let root = slot.worktree.root();
            items.push(
                PickerItem::new(index, slot.name.clone())
                    .with_detail(root.display().to_string())
                    .with_accent(slot.color)
                    .with_dots(running_dots(root)),
            );
            let Some((_, worktrees)) = collected.iter().find(|(i, _)| *i == index) else {
                continue;
            };
            for (worktree, status) in worktrees {
                let branch = worktree.branch.clone().unwrap_or_else(|| "(detached)".to_string());
                let mut meta = String::new();
                if worktree.path == root {
                    meta.push_str("✓ ");
                }
                if status.ahead > 0 {
                    meta.push_str(&format!("↑{} ", status.ahead));
                }
                if status.behind > 0 {
                    meta.push_str(&format!("↓{} ", status.behind));
                }
                if status.dirty {
                    meta.push('●');
                }
                let name = worktree
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                let detail = format!("{name}  {}", meta.trim_end()).trim_end().to_string();
                items.push(
                    PickerItem::new(1000 + rows.len(), format!("    ⎇ {branch}"))
                        .with_detail(detail)
                        .with_dots(running_dots(&worktree.path)),
                );
                rows.push(worktree.path.clone());
            }
        }
        (items, rows)
    }

    /// ⌘O の worktree 行を確定: 現プロジェクトなら何もしない・レールに居れば切替・
    /// 居なければ**このウィンドウのレールに開く**（M10-2 でウィンドウモデルを更新。新窓は右クリック明示）。
    pub(crate) fn open_worktree_target(
        &mut self,
        path: PathBuf,
        branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) =
            self.project_sessions.projects.iter().position(|slot| slot.worktree.root() == path.as_path())
        {
            self.switch_project(index, window, cx);
            return;
        }
        let host = match self.active_worktree() {
            Some(worktree) => worktree.host().clone(),
            None => host::LocalHost::shared(),
        };
        self.open_folder_in_rail(host, path, branch, cx);
    }

    // ── テーマセレクタ（Picker・ライブプレビュー付き。⌘⇧T・M3） ──

    /// テーマセレクタを開く。組み込み + ユーザーテーマを Picker に並べ、選択移動で即プレビューする。
    pub(crate) fn open_theme_selector(&mut self, _: &ThemeSelector, window: &mut Window, cx: &mut Context<Self>) {
        let themes = theme_core::available_themes(self.themes_dir().as_deref());
        let items = themes
            .iter()
            .enumerate()
            .map(|(id, (name, source))| {
                let detail = match source {
                    ThemeSource::BuiltIn(_) => i18n::t!("theme.builtin"),
                    ThemeSource::User(_) => i18n::t!("theme.user"),
                };
                PickerItem::new(id, name.clone()).with_detail(detail)
            })
            .collect();
        self.overlays.picker_themes = themes;
        self.overlays.theme_before_preview = Some(self.theme.clone());
        self.open_picker(PickerMode::Themes, i18n::t!("theme.picker_placeholder"), items, window, cx);
    }

    /// テーマ保存ディレクトリ（`state.json` と同じ Shirushi 設定フォルダの `themes/`）。
    pub(crate) fn themes_dir(&self) -> Option<PathBuf> {
        self.persistence.state_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|dir| dir.join("themes"))
    }

    /// テーマを即時適用する（自身のクローム + エディタ + Agent パネル + Picker へ波及）。
    pub(crate) fn apply_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        for (index, session) in self.project_sessions.sessions.iter().enumerate() {
            for tab in &session.tabs {
                tab.editor.update(cx, |editor, cx| editor.set_theme(theme.clone(), cx));
            }
            if let Some(split) = &session.split_editor {
                split.update(cx, |editor, cx| editor.set_theme(theme.clone(), cx));
            }
            session
                .agent_panel
                .update(cx, |panel, cx| panel.set_theme(theme.clone(), cx));
            if let Some(panel) = &session.search_panel {
                let accent = self
                    .project_sessions
                    .projects
                    .get(index)
                    .map(|slot| slot.color)
                    .unwrap_or_else(|| project_color(index));
                panel.update(cx, |panel, cx| panel.set_theme(theme.clone(), accent, cx));
            }
            session
                .terminal_dock
                .update(cx, |dock, cx| dock.set_theme(theme.clone(), cx));
        }
        if let Some(picker) = &self.overlays.picker {
            picker.update(cx, |picker, cx| picker.set_theme(theme.clone(), cx));
        }
        cx.notify();
    }

    /// ハイライト移動でプレビュー適用する（保存しない）。
    pub(crate) fn preview_theme(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some((_, source)) = self.overlays.picker_themes.get(id) {
            if let Ok(theme) = Theme::load(source) {
                self.apply_theme(theme, cx);
            }
    }
}
    /// テーマを確定する（適用 + 設定へ theme 名を保存＝再起動でも効く）。
    pub(crate) fn commit_theme(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some((_, source)) = self.overlays.picker_themes.get(id).cloned() else {
            return;
        };
        let Ok(theme) = Theme::load(&source) else {
            return;
        };
        let name = theme.name.to_string();
        self.apply_theme(theme, cx);
        self.overlays.theme_before_preview = None;
        if let Some(path) = settings_core::user_settings_path() {
            if let Err(error) =
                settings_core::persist_user_value(&path, "theme", serde_json::Value::String(name))
            {
                eprintln!("テーマの保存に失敗: {error:#}");
            }
        }
    }

    pub(crate) fn open_picker(
        &mut self,
        mode: PickerMode,
        placeholder: String,
        items: Vec<PickerItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let picker = cx.new(|cx| Picker::new(placeholder, items, theme, accent, cx));
        window.focus(&picker.read(cx).focus_handle(), cx);
        self.overlays.picker_observation = Some(cx.subscribe_in(&picker, window, Self::on_picker_event));
        self.overlays.picker_mode = mode;
        self.overlays.picker = Some(picker);
        cx.notify();
    }

    pub(crate) fn on_picker_event(
        &mut self,
        _picker: &Entity<Picker>,
        event: &PickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // テーマセレクタのみ、ハイライト移動で即プレビュー（ライブプレビュー）。
            PickerEvent::Highlighted(id) => {
                if self.overlays.picker_mode == PickerMode::Themes {
                    self.preview_theme(*id, cx);
                }
            }
            PickerEvent::Confirmed(id) => {
                let id = *id;
                let mode = self.overlays.picker_mode;
                self.close_picker(window, cx);
                match mode {
                    PickerMode::Files => {
                        if let Some(path) = self.overlays.picker_files.get(id).cloned() {
                            self.record_nav_position(cx); // ⌘P もナビ履歴へ
                            self.open_file(path, window, cx);
                        }
                    }
                    PickerMode::Projects => {
                        // id >= 1000 は worktree 行（M12-12）: レール切替 or 新窓で開く。
                        if id >= 1000 {
                            if let Some(path) = self.picker_worktree_rows.get(id - 1000).cloned() {
                                self.open_worktree_target(path, None, window, cx);
                            }
                        } else {
                            self.switch_project(id, window, cx);
                        }
                    }
                    PickerMode::Themes => self.commit_theme(id, cx),
                    PickerMode::Symbols => {
                        if let (Some(row), Some(editor)) =
                            (self.picker_symbol_rows.get(id).copied(), self.active_editor())
                        {
                            self.record_nav_position(cx);
                            editor.update(cx, |view, cx| view.reveal_position(row, 0, cx));
                        }
                    }
                    PickerMode::WorkspaceSymbols => {
                        if let Some((path, line, character)) =
                            self.picker_workspace_symbols.get(id).cloned()
                        {
                            self.record_nav_position(cx);
                            self.open_file_then(path, window, cx, move |editor, cx| {
                                editor.reveal_lsp_position(line, character, cx)
                            });
                        }
                    }
                    PickerMode::Commands => {
                        // パレットは既に閉じた（上の close_picker）ので、フォーカスは
                        // エディタへ戻っている = Editor/Workspace コンテキストで解決される。
                        if let Some(entry) = COMMAND_REGISTRY.get(id) {
                            match cx.build_action(entry.action_name, None) {
                                Ok(action) => window.dispatch_action(action, cx),
                                Err(error) => eprintln!(
                                    "パレット: {} を解決できない: {error}",
                                    entry.action_name
                                ),
                            }
                        }
                    }
                    PickerMode::SshHosts => {
                        // 前半 id = 最近のリモートプロジェクト（履歴・直接接続・#5）。
                        if let Some(uri) = self.picker_ssh_recent.get(id).cloned() {
                            self.connect_ssh_and_open(uri, cx);
                        } else {
                            // 後半 id = config ホスト（recent 分ずらす）+ 末尾の手入力。
                            let host_id = id - self.picker_ssh_recent.len();
                            match self.picker_ssh_hosts.get(host_id) {
                                Some(host) => {
                                    let alias = host.alias.clone();
                                    // 前回パスがあれば即接続（#2d・打たずに繋がる）。無ければパス入力へ。
                                    let last_path = self.persistence.storage.as_ref().and_then(|storage| {
                                        storage.host_last_path(&alias).ok().flatten()
                                    });
                                    match last_path {
                                        Some(path) => self
                                            .connect_ssh_and_open(format!("ssh://{alias}{path}"), cx),
                                        // 前回パスが無ければ home に直接接続（標準 SSH と同じ「ホスト選ぶ→home」・#5）。
                                        None => {
                                            self.connect_ssh_and_open(format!("ssh://{alias}"), cx)
                                        }
                                    }
                                }
                                // 末尾の「手入力」= 空の ssh:// 入力バー。
                                None => {
                                    self.open_ssh_input_seeded("ssh://".to_string(), window, cx)
                                }
                            }
                        }
                    }
                    PickerMode::ThreadHistory => {
                        if let Some((thread_id, name, color_index)) =
                            self.picker_history.get(id).cloned()
                        {
                            if !self.chrome.show_right {
                                self.chrome.show_right = true; // Agent ドックを開く
                            }
                            let panel = self.agent_panel.clone();
                            panel.update(cx, |panel, cx| {
                                panel.open_thread_from_history(
                                    &thread_id,
                                    &name,
                                    color_index as usize,
                                    cx,
                                )
                            });
                            cx.notify();
                        }
                    }
                }
            }
            PickerEvent::Dismissed => {
                // テーマセレクタを中止したらプレビューを元へ戻す。
                if self.overlays.picker_mode == PickerMode::Themes {
                    if let Some(theme) = self.overlays.theme_before_preview.take() {
                        self.apply_theme(theme, cx);
                    }
                }
                self.close_picker(window, cx);
            }
        }
    }

    pub(crate) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlays.picker = None;
        self.overlays.picker_observation = None;
        match self.active_editor() {
            Some(editor) => {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            None => window.focus(&self.focus_handle, cx),
        }
        cx.notify();
    }

    // ── プロジェクト横断検索パネル（⌘⇧F・M6） ──

    pub(crate) fn search_return_focus(&self, cx: &App) -> FocusHandle {
        self.active_editor()
            .map(|editor| editor.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.focus_handle.clone())
    }

    pub(crate) fn install_search_panel(
        &mut self,
        panel: Entity<SearchPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        PanelRegistry::bind_search(&panel, cx);
        let focus = panel.read(cx).focus_handle();
        self.search_panel = Some(panel);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn show_search_results(
        &mut self,
        query: String,
        results: Vec<search::FileMatch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.active_slot() else {
            return;
        };
        let host = slot.worktree.host().clone();
        let root = slot.worktree.root().to_path_buf();
        let theme = self.theme.clone();
        let accent = self.accent();
        let return_focus = self.search_return_focus(cx);
        let panel = cx.new(|cx| {
            SearchPanel::with_results(
                host,
                root,
                query,
                results,
                theme,
                accent,
                return_focus,
                cx,
            )
        });
        self.install_search_panel(panel, window, cx);
    }

    pub(crate) fn on_search_panel_event(
        &mut self,
        panel: Entity<SearchPanel>,
        event: &SearchPanelEvent,
        cx: &mut Context<Self>,
    ) {
        let session_index = self
            .project_sessions
            .sessions
            .iter()
            .position(|session| session.search_panel.as_ref() == Some(&panel))
            .unwrap_or(self.project_sessions.active);
        match event {
            SearchPanelEvent::OpenMatch { path, line, column } => {
                self.project_sessions.sessions[session_index].pending_navigation =
                    Some((path.clone(), *line, *column));
                self.project_sessions.sessions[session_index].search_panel = None;
            }
            SearchPanelEvent::Dismissed => self.project_sessions.sessions[session_index].search_panel = None,
        }
        cx.notify();
    }

    pub(crate) fn open_project_search(&mut self, _: &ProjectSearch, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = &self.search_panel {
            window.focus(&panel.read(cx).focus_handle(), cx);
            return;
        }
        let Some(slot) = self.active_slot() else {
            return;
        };
        let host = slot.worktree.host().clone();
        let root = slot.worktree.root().to_path_buf();
        let theme = self.theme.clone();
        let accent = self.accent();
        let return_focus = self.search_return_focus(cx);
        let panel = cx.new(|cx| SearchPanel::new(host, root, theme, accent, return_focus, cx));
        self.install_search_panel(panel, window, cx);
    }

    // ── ⌘F バッファ内検索/置換（M10） ──

    pub(crate) fn open_buffer_search(&mut self, _: &BufferSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_buffer_search_impl(false, window, cx);
    }

    pub(crate) fn open_buffer_replace(&mut self, _: &BufferReplace, window: &mut Window, cx: &mut Context<Self>) {
        self.open_buffer_search_impl(true, window, cx);
    }

    /// ⌘F / ⌥⌘F。閉じていれば現在位置を保存して開く（単一行の選択があればクエリ初期値に）。
    /// 開いていればフォーカスを付け直すだけ（⌥⌘F は置換行も出す）。
    pub(crate) fn open_buffer_search_impl(&mut self, with_replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let seed = {
            let view = editor.read(cx);
            view.buffer()
                .selections()
                .first()
                .copied()
                .filter(|selection| !selection.is_empty())
                .map(|selection| view.buffer().text_range(selection.range()))
                .filter(|text| !text.contains('\n') && text.len() <= 200)
        };
        match &mut self.buffer_search {
            Some(state) => {
                state.show_replace |= with_replace;
                state.editing_replace = with_replace;
                if let Some(seed) = seed {
                    state.query = seed;
                }
            }
            None => {
                self.buffer_search = Some(BufferSearchState {
                    query: seed.unwrap_or_default(),
                    replace: String::new(),
                    case_sensitive: false,
                    is_regex: false,
                    show_replace: with_replace,
                    editing_replace: with_replace,
                    matches: Vec::new(),
                    truncated: false,
                    current: 0,
                    error: None,
                    computed_for: None,
                    saved_position: editor.read(cx).position_snapshot(),
                    focus: cx.focus_handle(),
                });
            }
        }
        if let Some(state) = self.buffer_search.as_ref() {
            window.focus(&state.focus.clone(), cx);
        }
        self.refresh_buffer_search(true, cx);
        cx.notify();
    }

    /// ⌘F バーを閉じる。`restore` = 開いた時の位置へ戻す（Esc）。×クリックは現在位置のまま閉じる。
    pub(crate) fn close_buffer_search(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.buffer_search.take() else {
            return;
        };
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| {
                editor.set_search_ranges(Vec::new(), cx);
                if restore {
                    editor.restore_position(&state.saved_position, cx);
                }
            });
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        } else {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    /// アクティブエディタが変わる操作（タブ切替・タブ閉じ・プロジェクト切替/再読込）では
    /// ⌘F バーを畳む（マッチはエディタ毎の状態なので持ち越さない。位置復帰もしない）。
    pub(crate) fn dismiss_buffer_search(&mut self, cx: &mut Context<Self>) {
        if self.buffer_search.take().is_some() {
            for tab in &self.tabs {
                tab.editor.update(cx, |editor, cx| editor.set_search_ranges(Vec::new(), cx));
            }
            cx.notify();
        }
    }

    /// ⌘F の検索を再計算する。(バッファ version, クエリ, トグル) が前回と同じならスキップ
    /// （エディタの observe は blink でも発火するため）。`reveal` = 現在マッチを選択して画面内へ。
    pub(crate) fn refresh_buffer_search(&mut self, reveal: bool, cx: &mut Context<Self>) {
        self.refresh_buffer_search_from(None, reveal, cx);
    }

    /// [`Self::refresh_buffer_search`] の anchor 指定版。`anchor` 以降で最初のマッチを現在マッチに
    /// する（None = 前回の現在マッチの開始位置 → 無ければエディタのキャレット位置）。
    pub(crate) fn refresh_buffer_search_from(
        &mut self,
        anchor_override: Option<usize>,
        reveal: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(state) = self.buffer_search.as_ref() else {
            return;
        };
        let (version, caret, text) = {
            let view = editor.read(cx);
            let caret = view.buffer().selections().first().map(|s| s.start()).unwrap_or(0);
            (view.buffer().version(), caret, view.buffer().text())
        };
        let key = (version, state.query.clone(), state.case_sensitive, state.is_regex);
        if state.computed_for.as_ref() == Some(&key) {
            if reveal {
                self.reveal_current_buffer_match(cx);
            }
            return;
        }
        let anchor = anchor_override
            .or_else(|| state.matches.get(state.current).map(|range| range.start))
            .unwrap_or(caret);
        let (ranges, truncated, error) =
            match search::SearchQuery::new(&state.query, state.is_regex, state.case_sensitive) {
                Ok(query) => {
                    let mut ranges = query.find_in(&text, BUFFER_SEARCH_MAX + 1);
                    let truncated = ranges.len() > BUFFER_SEARCH_MAX;
                    ranges.truncate(BUFFER_SEARCH_MAX);
                    (ranges, truncated, None)
                }
                Err(error) => (Vec::new(), false, Some(SharedString::from(format!("{error:#}")))),
            };
        // anchor 以降で最初のマッチを現在に（末尾を越えたら先頭へ回る）。
        let current = {
            let position = ranges.partition_point(|range| range.start < anchor);
            if position >= ranges.len() { 0 } else { position }
        };
        if let Some(state) = self.buffer_search.as_mut() {
            state.matches = ranges.clone();
            state.truncated = truncated;
            state.error = error;
            state.current = current;
            state.computed_for = Some(key);
        }
        editor.update(cx, |editor, cx| editor.set_search_ranges(ranges, cx));
        if reveal {
            self.reveal_current_buffer_match(cx);
        }
        cx.notify();
    }

    /// 現在マッチを選択して画面内へ（可視域外なら中央へスクロール）。
    pub(crate) fn reveal_current_buffer_match(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(range) = self
            .buffer_search
            .as_ref()
            .and_then(|state| state.matches.get(state.current).cloned())
        else {
            return;
        };
        editor.update(cx, |editor, cx| editor.select_byte_range(range, cx));
    }

    /// 次/前のマッチへ（Enter / ⇧Enter・‹ ›。端で回る）。
    pub(crate) fn step_buffer_match(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.refresh_buffer_search(false, cx);
        let Some(state) = self.buffer_search.as_mut() else {
            return;
        };
        let len = state.matches.len();
        if len == 0 {
            return;
        }
        state.current = (state.current as isize + delta).rem_euclid(len as isize) as usize;
        self.reveal_current_buffer_match(cx);
        cx.notify();
    }

    /// 現在マッチ 1 件を置換して次のマッチへ進む。
    pub(crate) fn replace_current_buffer_match(&mut self, cx: &mut Context<Self>) {
        self.refresh_buffer_search(false, cx);
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some((range, replacement)) = self.buffer_search.as_ref().and_then(|state| {
            state
                .matches
                .get(state.current)
                .cloned()
                .map(|range| (range, state.replace.clone()))
        }) else {
            return;
        };
        editor.update(cx, |editor, cx| {
            editor.replace_ranges(&[range.clone()], &replacement, cx)
        });
        // 挿入末尾の直後を anchor に再計算（置換文字列がパターンに再マッチしても足踏みしない）。
        self.refresh_buffer_search_from(Some(range.start + replacement.len()), true, cx);
    }

    /// 全マッチを 1 Transaction で置換する（undo 一発で全部戻る）。表示上限より多くても全件対象。
    pub(crate) fn replace_all_buffer_matches(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some((query_text, is_regex, case_sensitive, replacement)) =
            self.buffer_search.as_ref().map(|state| {
                (state.query.clone(), state.is_regex, state.case_sensitive, state.replace.clone())
            })
        else {
            return;
        };
        if query_text.is_empty() {
            return;
        }
        let Ok(query) = search::SearchQuery::new(&query_text, is_regex, case_sensitive) else {
            return;
        };
        editor.update(cx, |editor, cx| {
            let text = editor.buffer().text();
            let ranges = query.find_in(&text, usize::MAX);
            if !ranges.is_empty() {
                editor.replace_ranges(&ranges, &replacement, cx);
            }
        });
        self.refresh_buffer_search(false, cx);
        cx.notify();
    }

    /// 大小区別トグル（⌘F バー。切替後は即再検索）。
    pub(crate) fn toggle_buffer_search_case(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.buffer_search.as_mut() {
            state.case_sensitive = !state.case_sensitive;
        }
        self.refresh_buffer_search(true, cx);
    }

    /// 正規表現トグル（⌘F バー。切替後は即再検索）。
    pub(crate) fn toggle_buffer_search_regex(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.buffer_search.as_mut() {
            state.is_regex = !state.is_regex;
        }
        self.refresh_buffer_search(true, cx);
    }

    /// ⌘F バーのキー入力（検索パネル・git パネルと同じ手書き入力の流儀）。
    pub(crate) fn on_buffer_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.buffer_search.as_ref() else {
            return;
        };
        let show_replace = state.show_replace;
        let editing_replace = state.editing_replace && show_replace;
        let modifiers = event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => self.close_buffer_search(true, window, cx),
            // ⌘Enter = 全置換（置換行が出ている時のみ）。
            "enter" if modifiers.platform => {
                if show_replace {
                    self.replace_all_buffer_matches(cx);
                }
            }
            "enter" if modifiers.shift => self.step_buffer_match(-1, cx),
            "enter" => {
                if editing_replace {
                    self.replace_current_buffer_match(cx);
                } else {
                    self.step_buffer_match(1, cx);
                }
            }
            "tab" if show_replace => {
                if let Some(state) = self.buffer_search.as_mut() {
                    state.editing_replace = !state.editing_replace;
                }
                cx.notify();
            }
            "backspace" => {
                if let Some(state) = self.buffer_search.as_mut() {
                    if editing_replace {
                        state.replace.pop();
                        cx.notify();
                    } else {
                        state.query.pop();
                        self.refresh_buffer_search(true, cx);
                    }
                }
            }
            // ⌘V: クリップボードをアクティブフィールドへ貼り付け。
            "v" if modifiers.platform => {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return;
                };
                if let Some(state) = self.buffer_search.as_mut() {
                    if editing_replace {
                        state.replace.push_str(&text);
                        cx.notify();
                    } else {
                        state.query.push_str(&text);
                        self.refresh_buffer_search(true, cx);
                    }
                }
            }
            _ => {
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }
                if let Some(state) = self.buffer_search.as_mut() {
                    if editing_replace {
                        state.replace.push_str(text);
                        cx.notify();
                    } else {
                        state.query.push_str(text);
                        self.refresh_buffer_search(true, cx);
                    }
                }
            }
        }
    }

    /// 開発用: アクティブエディタの (row, col)（0 始まり）へキャレットを置き `text` をタイプする。
    /// [`EditorInputEvent::Typed`] が発火する＝補完の自動トリガをオフスクリーンで検証する用。
    #[cfg(debug_assertions)]
    pub fn debug_type_probe(
        &mut self,
        row: usize,
        column: usize,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        editor.update(cx, |view, cx| {
            view.reveal_position(row, column, cx);
            view.insert_text(&text, cx);
        });
    }

    /// 開発用: インライン命名を Enter 確定する（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_confirm_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.explorer_naming(cx).is_some() {
            self.confirm_naming(window, cx);
        }
    }

    /// 開発用: (row,col) にキャレットを置いて rename を実行する（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_rename_probe(
        &mut self,
        row: usize,
        column: usize,
        new_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.reveal_position(row, column, cx));
        }
        self.perform_rename(new_name, window, cx);
    }

    /// 開発用: (row,col) で ⌘. を開く（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_code_actions_probe(&mut self, row: usize, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.reveal_position(row, column, cx));
        }
        self.open_code_actions(&CodeActions, window, cx);
    }

    /// 開発用: ⌘. ポップアップの選択中アクションを確定して保存する（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_confirm_code_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_actions.is_some() {
            self.confirm_code_action(window, cx);
        }
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.save_now(cx));
        }
    }

    // 開発用: (row,col) で ⇧F12 参照検索を実行する（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_references_probe(&mut self, row: usize, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.reveal_position(row, column, cx));
        }
        self.find_references(&FindReferences, window, cx);
    }

    // ターミナルの file:line リンク（M13）。相対パスはアクティブプロジェクトの root 基準。
    // subscribe に window が無いので pending_transient_tab と同様「次の render で消化」する。

    /// スレッド履歴を開く（#5）。DB の全スレッド（アーカイブ含む・updated_at 降順）を Picker に出す。
    /// 行頭●= スレッド色・detail = プロジェクト / ⎇ branch / トークン累計。確定で復元してアクティブに。
    pub(crate) fn open_thread_history(&mut self, _: &ThreadHistory, window: &mut Window, cx: &mut Context<Self>) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        let threads = storage.load_all_threads().unwrap_or_default();
        let mut history = Vec::new();
        let mut items = Vec::new();
        for (id, name, color_index, project, branch, tokens_used, archived) in threads {
            let mut detail = String::new();
            if !project.is_empty() {
                detail.push_str(&project);
            }
            if let Some(branch) = &branch {
                detail.push_str(&format!("  ⎇ {branch}"));
            }
            if tokens_used > 0 {
                detail.push_str(&format!("  Σ {:.1}k", tokens_used as f32 / 1000.0));
            }
            if archived {
                detail.push_str("  ·閉");
            }
            let mut item = PickerItem::new(history.len(), name.clone())
                .with_accent(theme_core::thread_color(color_index as usize));
            if !detail.is_empty() {
                item = item.with_detail(detail);
            }
            items.push(item);
            history.push((id, name, color_index));
        }
        self.picker_history = history;
        self.open_picker(
            PickerMode::ThreadHistory,
            i18n::t!("agent.history_placeholder"),
            items,
            window,
            cx,
        );
    }
}
