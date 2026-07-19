/// Rail 上の project metadata と、遅延復元に必要なファイル一覧。
struct ProjectSlot {
    worktree: Rc<Worktree>,
    name: SharedString,
    /// 描画中に git process/RPC を起動しないための現在ブランチ cache。
    branch: Option<String>,
    color: Hsla,
    explorer: ExplorerProject,
    /// このプロジェクトで開いているタブのファイル一覧（左から順・M10 複数タブ）。
    /// アクティブ session の `EditorArea.tabs` から [`Workspace::sync_active_slot`] で同期する。
    open_files: Vec<PathBuf>,
    active_file: usize,
    /// `.shirushi/settings.json` の絵文字アイコン（None = 頭文字モノグラム）。
    icon: Option<SharedString>,
    /// リンク worktree ならブランチ名。通常/メイン slot は None。
    worktree_branch: Option<String>,
}

impl ProjectSlot {
    fn refresh(&mut self) {
        self.explorer.refresh(&self.worktree);
    }

    /// Explorer の Render はこのキャッシュだけを読み、FS/RPC を呼ばない。
    fn listed_dir(&self, dir: &Path) -> Vec<project::Entry> {
        self.explorer.listed_dir(dir)
    }
}

/// 1 project の長寿命 UI / controller 群。非アクティブ時も Entity と process を保持する。
pub struct ProjectSession {
    editor_area: EditorArea,
    agent_panel: Entity<AgentPanel>,
    explorer: Entity<Explorer>,
    search_panel: Option<Entity<SearchPanel>>,
    repository: RepositoryController,
    git_panel: Entity<GitPanel>,
    terminal_dock: Entity<TerminalDock>,
    agent_active: bool,
    picker_worktree_rows: Vec<PathBuf>,
    picker_ssh_hosts: Vec<host::SshConfigHost>,
    picker_ssh_recent: Vec<String>,
    picker_history: Vec<(String, String, i64)>,
    todo_panel: Entity<TodoPanel>,
    pending_open_history: bool,
    agent_touched: HashMap<PathBuf, Hsla>,
    waiting_thread: Option<(SharedString, Hsla)>,
    _watch: Option<project::Watch>,
    _watch_pump: Option<gpui::Task<()>>,
}

/// Rail metadata と長寿命 session を同じ添字で管理する非公開 owner。
struct ProjectSessions {
    projects: Vec<ProjectSlot>,
    active: usize,
    sessions: Vec<ProjectSession>,
}

struct RepositoryController {
    status: HashMap<PathBuf, StatusKind>,
    refresh_generation: u32,
}

impl Workspace {
    fn create_project_session(
        slot: Option<&ProjectSlot>,
        theme: Theme,
        explorer_view: ExplorerView,
        storage: Option<storage::Storage>,
        cx: &mut Context<Self>,
    ) -> ProjectSession {
        let agent_panel = cx.new(|cx| AgentPanel::new(theme.clone(), cx));
        if let Some(storage) = storage {
            agent_panel.update(cx, |panel, cx| panel.set_storage(storage, cx));
        }
        let terminal_launch = Self::terminal_launch_for(slot);
        let terminal_dock = cx.new(|_| TerminalDock::new(terminal_launch, theme.clone()));
        let explorer = cx.new(|_| Explorer::new(explorer_view));
        let git_panel = cx.new(GitPanel::new);
        let accent = slot.map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let todo_panel = cx.new(|_| TodoPanel::new(theme.clone(), accent));
        PanelRegistry::bind_session(
            &agent_panel,
            &explorer,
            &git_panel,
            &terminal_dock,
            &todo_panel,
            cx,
        );
        ProjectSession {
            editor_area: EditorArea::new(),
            agent_panel,
            explorer,
            search_panel: None,
            repository: RepositoryController { status: HashMap::new(), refresh_generation: 0 },
            git_panel,
            terminal_dock,
            agent_active: false,
            picker_worktree_rows: Vec::new(),
            picker_ssh_hosts: Vec::new(),
            picker_ssh_recent: Vec::new(),
            picker_history: Vec::new(),
            todo_panel,
            pending_open_history: false,
            agent_touched: HashMap::new(),
            waiting_thread: None,
            _watch: None,
            _watch_pump: None,
        }
    }

    /// プロジェクトのルート群からワークスペースを組み立てる。開けないルートはスキップ。
    pub fn new(
        roots: Vec<PathBuf>,
        theme: Theme,
        state_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_sources(
            roots.into_iter().map(ProjectSource::local).collect(),
            theme,
            state_path,
            cx,
        )
    }

    /// local/remote の host と root を保ったままワークスペースを組み立てる。
    pub fn new_sources(
        sources: Vec<ProjectSource>,
        theme: Theme,
        state_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_sources_with_active(sources, 0, theme, state_path, cx)
    }

    /// 復元時の active index を含めてワークスペースを組み立てる。
    /// リモートプロジェクトの窓色をローカル DB（storage）から解決する（M13 #3b）。
    /// `.shirushi` はリモート側にあり identity に使えないため、ホスト識別子 → 色をローカルに持つ。
    /// 初回は識別子から安定に 1 色を焼き付け、以後は同じ色（開き直し・レール並び順が変わっても不変）。
    fn apply_remote_host_colors(&mut self) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        for slot in &mut self.project_sessions.projects {
            if !slot.worktree.host().is_remote() {
                continue;
            }
            let key = slot.worktree.host().display_name().to_string();
            let color = match storage.host_color(&key) {
                Ok(Some(color)) => color,
                Ok(None) => {
                    // 初回: 識別子のハッシュで IDENTITY パレットから安定に 1 色選び、焼き付ける。
                    let palette = theme_core::IDENTITY_PALETTE_HEXES;
                    let index = key
                        .bytes()
                        .fold(0u32, |acc, byte| acc.wrapping_mul(31).wrapping_add(byte as u32))
                        as usize
                        % palette.len();
                    let color = palette[index];
                    let _ = storage.set_host_color(&key, color);
                    color
                }
                Err(_) => continue,
            };
            slot.color = theme_core::color_from_hex(color);
        }
    }

    pub fn new_sources_with_active(
        sources: Vec<ProjectSource>,
        active: usize,
        theme: Theme,
        state_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut projects = Vec::new();
        for source in sources {
            let (host, root) = source.into_parts();
            match Worktree::with_host(host, &root) {
                Ok(worktree) => {
                    let index = projects.len();
                    // `.shirushi/settings.json` の color(#hex)/icon(絵文字) を反映（M12-11）。
                    let identity = read_project_identity(worktree.root());
                    let mut slot = ProjectSlot {
                        name: worktree.name().into(),
                        branch: None,
                        color: identity.0.unwrap_or_else(|| project_color(index)),
                        worktree: Rc::new(worktree),
                        explorer: ExplorerProject::default(),
                        open_files: Vec::new(),
                        active_file: 0,
                        icon: identity.1,
                        worktree_branch: None,
                    };
                    slot.refresh();
                    projects.push(slot);
                }
                Err(error) => eprintln!("プロジェクトを開けない（スキップ）: {error:#}"),
            }
        }
        // 開発用: SHIRUSHI_EXPLORER_UP=1 で先頭プロジェクトをルート直上（隣リポジトリ一覧）から撮る。
        if std::env::var_os("SHIRUSHI_EXPLORER_UP").is_some() {
            if let Some(slot) = projects.first_mut() {
                slot.explorer.current_dir = slot.worktree.root().parent().map(Path::to_path_buf);
            }
        }
        // 開発用フックの値は projects が move される前に計算しておく。
        let mut explorer_context_menu = std::env::var_os("SHIRUSHI_CONTEXT_MENU").and_then(|_| {
            projects.first().map(|slot| ExplorerContextMenu {
                path: slot.worktree.root().to_path_buf(),
                is_dir: true,
                position: point(px(120.0), px(210.0)),
            })
        });
        // 開発用: SHIRUSHI_SEARCH_PANEL=1 で「fn」を横断検索した結果パネルを開いた状態で撮る。
        let mut search_probe = std::env::var_os("SHIRUSHI_SEARCH_PANEL").and_then(|_| {
            projects.first().map(|slot| {
                let results = search::SearchQuery::new("fn", false, false)
                    .map(|query| {
                        query.search_project_on(
                            slot.worktree.host().as_ref(),
                            slot.worktree.root(),
                            5_000,
                            300,
                        )
                    })
                    .unwrap_or_default();
                results
            })
        });
        let active = active.min(projects.len().saturating_sub(1));
        let focus_handle = cx.focus_handle();
        let explorer_view = match std::env::var("SHIRUSHI_EXPLORER_VIEW").as_deref() {
            Ok("icons") => ExplorerView::Icons,
            Ok("columns") => ExplorerView::Columns,
            _ => ExplorerView::Tree,
        };
        let mut sessions = Vec::with_capacity(projects.len().max(1));
        for index in 0..projects.len().max(1) {
            let agent_panel = cx.new(|cx| AgentPanel::new(theme.clone(), cx));
            let terminal_launch = Self::terminal_launch_for(projects.get(index));
            let terminal_dock = cx.new(|_| TerminalDock::new(terminal_launch, theme.clone()));
            let explorer = cx.new(|_| Explorer::new(explorer_view));
            let git_panel = cx.new(GitPanel::new);
            let accent = projects.get(index).map(|slot| slot.color).unwrap_or_else(|| project_color(0));
            let todo_panel = cx.new(|_| TodoPanel::new(theme.clone(), accent));
            PanelRegistry::bind_session(
                &agent_panel,
                &explorer,
                &git_panel,
                &terminal_dock,
                &todo_panel,
                cx,
            );
            if index == active {
                if let Some(menu) = explorer_context_menu.take() {
                    explorer.update(cx, |explorer, cx| explorer.show_context_menu(menu, cx));
                }
            }
            let search_panel = if index == active {
                search_probe.take().and_then(|results| {
                    let slot = projects.get(index)?;
                    let panel = cx.new(|cx| {
                        SearchPanel::with_results(
                            slot.worktree.host().clone(),
                            slot.worktree.root().to_path_buf(),
                            "fn".to_string(),
                            results,
                            theme.clone(),
                            slot.color,
                            focus_handle.clone(),
                            cx,
                        )
                    });
                    PanelRegistry::bind_search(&panel, cx);
                    Some(panel)
                })
            } else {
                None
            };
            sessions.push(ProjectSession {
                editor_area: EditorArea::new(),
                agent_panel,
                explorer,
                search_panel,
                repository: RepositoryController { status: HashMap::new(), refresh_generation: 0 },
                git_panel,
                terminal_dock,
                agent_active: false,
                picker_worktree_rows: Vec::new(),
                picker_ssh_hosts: Vec::new(),
                picker_ssh_recent: Vec::new(),
                picker_history: Vec::new(),
                todo_panel,
                pending_open_history: false,
                agent_touched: HashMap::new(),
                waiting_thread: None,
                _watch: None,
                _watch_pump: None,
            });
        }
        let accent = projects.get(active).map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let settings_view = cx.new(|cx| settings::SettingsView::new(theme.clone(), accent, cx));
        PanelRegistry::bind_settings(&settings_view, cx);
        let mut workspace = Workspace {
            project_sessions: ProjectSessions { projects, active, sessions },
            theme,
            focus_handle,
            chrome: ChromeState {
                show_left: true,
                show_right: true,
                show_bottom: false,
                show_settings: std::env::var_os("SHIRUSHI_SETTINGS").is_some()
                    || !settings::get(cx).onboarded,
                settings_view,
                pending_settings_command: None,
                confetti: std::env::var_os("SHIRUSHI_CONFETTI").is_some(),
                agent_width: AGENT_DOCK_WIDTH,
                resizing_agent: false,
                resize_start_x: 0.0,
                resize_start_width: 0.0,
                explorer_width: DOCK_WIDTH,
                resizing_explorer: false,
                should_move_window: false,
            },
            overlays: WorkspaceOverlays {
                picker: None,
                picker_mode: PickerMode::Files,
                picker_files: Vec::new(),
                picker_themes: Vec::new(),
                theme_before_preview: None,
                picker_observation: None,
                color_picker: None,
                rail_menu: None,
                ssh_input: None,
                ssh_connecting: false,
                add_project_dialog_open: false,
                pending_project_switch: None,
            },
            notifications: NotificationCenter { toasts: Vec::new(), toast_gen: 0 },
            persistence: WorkspacePersistence { state_path, storage: None },
            updater: UpdateController { status: None },
        };
        workspace.refresh_git_status(cx); // ツリー/タブの git 色分け用
        // 開発用: SHIRUSHI_GIT_PANEL=1 で git 操作パネル（ソース管理）を開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_GIT_PANEL").is_some() {
            workspace.git_panel.update(cx, |panel, cx| panel.set_open(true, cx));
            workspace.refresh_git_status(cx);
        }
        // 開発用: SHIRUSHI_BRANCH_MENU=1 で branch/worktree メニューを開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_BRANCH_MENU").is_some() {
            workspace.toggle_branch_menu(point(px(90.), px(44.)), cx);
        }
        // 開発用: SHIRUSHI_TERMINAL=1 で下ドックのターミナルを開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_TERMINAL").is_some() {
            workspace.chrome.show_bottom = true;
            workspace.terminal_dock.update(cx, |dock, cx| {
                dock.ensure_active(cx);
            });
        }
        // 開発用: SHIRUSHI_COLOR_PICKER=1（or hex 文字列）で色ピッカーを開いた状態で撮る（Peacock 拡張の検証）。
        if let Ok(probe) = std::env::var("SHIRUSHI_COLOR_PICKER") {
            let hex = if probe == "1" { String::new() } else { probe.trim_start_matches('#').to_string() };
            workspace.overlays.color_picker = Some(ColorPickerState {
                project_index: workspace.project_sessions.active,
                position: point(px(RAIL_WIDTH), px(12.)),
                hex,
                focus: cx.focus_handle(),
            });
        }
        // 開発用: SHIRUSHI_RAIL_MENU でレール項目の右クリックメニューを開いた状態で撮る（M10-2）。
        // 値: 1=通常 / confirm-worktree / confirm-branch=破壊的操作の二段確認 armed 状態。
        // worktree/ブランチ削除行も見せるため、アクティブスロットを worktree タブ扱いにする。
        if let Ok(mode) = std::env::var("SHIRUSHI_RAIL_MENU") {
            let active = workspace.project_sessions.active;
            if let Some(slot) = workspace.project_sessions.projects.get_mut(active) {
                slot.worktree_branch = Some("feature/login".to_string());
            }
            let confirm = match mode.as_str() {
                "confirm-worktree" => Some(RailMenuAction::RemoveWorktree),
                "confirm-branch" => Some(RailMenuAction::DeleteBranch),
                _ => None,
            };
            workspace.overlays.rail_menu = Some(RailMenuState {
                project_index: workspace.project_sessions.active,
                position: point(px(RAIL_WIDTH), px(12.)),
                confirm,
            });
        }
        // 開発用: SHIRUSHI_COMPLETION=1 で補完ポップアップ（サンプル候補）を開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_COMPLETION").is_some() {
            let sample = |label: &str, detail: &str, kind: &str| CompletionItem {
                label: SharedString::from(label.to_string()),
                insert_text: label.to_string(),
                detail: Some(SharedString::from(detail.to_string())),
                kind: SharedString::from(kind.to_string()),
            };
            workspace.completion = Some(CompletionState {
                items: vec![
                    sample("push_str", "fn(&mut self, string: &str)", "fn"),
                    sample("push", "fn(&mut self, ch: char)", "fn"),
                    sample("PathBuf", "struct std::path::PathBuf", "type"),
                    sample("parse", "fn(&self) -> Result<F>", "fn"),
                    sample("println!", "macro", "•"),
                ],
                prefix: String::new(),
                selected: 1,
                position: point(px(380.), px(210.)),
                focus: cx.focus_handle(),
            });
        }
        // 開発用: SHIRUSHI_NAMING=1 でルートへの新規ファイル命名入力を開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_NAMING").is_some() {
            if let Some(root) = workspace.active_worktree().map(|worktree| worktree.root().to_path_buf()) {
                workspace.explorer.update(cx, |explorer, cx| {
                    explorer.set_naming(
                        ExplorerNaming {
                            kind: NamingKind::NewFile,
                            parent: root,
                            target: None,
                            value: "new_file.rs".to_string(),
                            focus: cx.focus_handle(),
                        },
                        cx,
                    )
                });
            }
        }
        workspace.update_agent_destination(cx); // 宛先チップにプロジェクト/ブランチを反映
        workspace.start_watcher(cx); // アクティブプロジェクトのファイル監視（M10 watch 基盤）
        workspace.loaded = true;
        workspace.schedule_update_check(cx); // 自動アップデートの確認（M13・90s 後に背景で）
        // 開発用: SHIRUSHI_UPDATE_PROBE="x.y.z" でチップ描画を直接確認（ネット不要）。
        if let Ok(version) = std::env::var("SHIRUSHI_UPDATE_PROBE") {
            if !version.is_empty() {
                workspace.updater.status = Some((
                    updater::UpdateInfo { version, dmg_url: String::new() },
                    UpdateState::Available,
                ));
            }
        }
        // ローカル永続化 DB（hot exit・M10）。SHIRUSHI_DB でパス上書き（検証用）。開けなくても起動は続行。
        let db_path = std::env::var("SHIRUSHI_DB")
            .map(PathBuf::from)
            .ok()
            .or_else(storage::default_db_path);
        if let Some(db_path) = db_path {
            match storage::Storage::open(&db_path) {
                Ok(handle) => {
                    // Agent パネルへも同じハンドルを渡す（スレッド永続化・M12-1。ワーカー 1 本を共有）。
                    for session in &workspace.project_sessions.sessions {
                        session
                            .agent_panel
                            .update(cx, |panel, cx| panel.set_storage(handle.clone(), cx));
                    }
                    workspace.persistence.storage = Some(handle);
                    // リモートプロジェクトの窓色をローカル DB から解決（M13 #3b）。
                    workspace.apply_remote_host_colors();
                }
                Err(error) => eprintln!("ローカル DB を開けない（hot exit 無効）: {error:#}"),
            }
        }
        // 設定が変わったら全エディタへ実効値を配り直す + 再描画（font_size/tab_size/soft_wrap の live 反映）。
        cx.observe_global::<settings::SettingsGlobal>(|workspace, cx| {
            workspace.apply_editor_settings(cx);
            cx.notify();
        })
        .detach();
        workspace.save_state(); // 起動時点で状態を書く（再起動復元のため）
        workspace
    }

    /// 起動後にタブ列を復元する。各プロジェクトの記録を slot へ流し込み（非アクティブは遅延復元）、
    /// アクティブプロジェクトのタブだけ実際に開く。
    pub fn restore_open_file(&mut self, restored: &[RestoredTabs], window: &mut Window, cx: &mut Context<Self>) {
        for (index, tabs) in restored.iter().enumerate() {
            if let Some(slot) = self.project_sessions.projects.get_mut(index) {
                slot.open_files = tabs.files.clone();
                slot.active_file = tabs.active;
            }
        }
        self.open_slot_files(window, cx);
        self.loaded = true;
    }

    /// 複数ファイルをアクティブプロジェクトのタブとして順に開く（最後がアクティブ）。
    /// 外部起点（起動時の複数ファイル指定・開発用フック）から使う。
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        for path in paths {
            self.open_file(path, window, cx);
        }
    }

    fn active_slot(&self) -> Option<&ProjectSlot> {
        self.project_sessions.projects.get(self.project_sessions.active)
    }

    /// 現在アクティブなタブのエディタ（無ければ `None`）。従来 `self.editor` を読んでいた箇所の置換。
    fn active_editor(&self) -> Option<Entity<EditorView>> {
        self.tabs.get(self.active_tab).map(|tab| tab.editor.clone())
    }

    /// アクティブタブのファイルパス。
    fn active_tab_path(&self) -> Option<PathBuf> {
        self.tabs.get(self.active_tab).map(|tab| tab.path.clone())
    }

    /// 現在のタブ列をアクティブ slot へ書き戻す（永続化・切替復元の真実源を同期）。
    fn sync_active_slot(&mut self) {
        // 一時タブ（diff 等）は永続化しない。
        let files: Vec<PathBuf> = self
            .tabs
            .iter()
            .filter(|tab| !tab.transient)
            .map(|tab| tab.path.clone())
            .collect();
        let active_file = self.active_tab.min(files.len().saturating_sub(1));
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.projects.get_mut(active) {
            slot.open_files = files;
            slot.active_file = active_file;
        }
    }

    /// diff などの一時タブを開く（永続化されない・読み取り専用は呼び出し側で設定済みの buffer を渡す）。
    fn open_transient_tab(
        &mut self,
        title_path: PathBuf,
        buffer: Buffer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == title_path) {
            // 既存の diff タブは作り直す（内容が古いため閉じて開き直し）。
            self.close_tab_at(index, window, cx);
        }
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let theme = self.theme.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let editor = cx.new(|cx| EditorView::new(buffer, theme, accent, cx));
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        let observation = cx.observe(&editor, Self::on_editor_changed);
        let input_subscription = cx.subscribe_in(&editor, window, Self::on_editor_typed);
        let hover_subscription = cx.subscribe_in(&editor, window, Self::on_editor_hover);
        self.tabs.push(EditorTab {
            path: title_path,
            editor,
            transient: true,
            _observation: observation,
            _input_subscription: input_subscription,
            _hover_subscription: hover_subscription,
        });
        self.active_tab = self.tabs.len() - 1;
        cx.notify();
    }

    /// アクティブ slot に記録された open_files を順に開き直す（タブ復元）。存在しないファイルは飛ばす。
    /// レール切替・ブランチ切替・起動復元で使う。
    fn open_slot_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // タブ列ごと入れ替わる（レール/ブランチ切替）ので ⌘F バー・hover は畳む。
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let (files, active_file) = match self.active_slot() {
            Some(slot) => (slot.open_files.clone(), slot.active_file),
            None => return,
        };
        let host = self.active_worktree().map(|worktree| worktree.host().clone());
        self.tabs.clear();
        self.active_tab = 0;
        for path in files {
            let exists = host.as_ref().map(|host| host.metadata(&path).is_ok()).unwrap_or(false);
            if exists {
                // 背景読み込みだと完了順でタブ順が崩れるため、復元だけは同期で開く。
                self.open_file_sync(path, window, cx);
            }
        }
        if active_file < self.tabs.len() {
            self.select_tab(active_file, window, cx);
        }
    }

    fn active_worktree(&self) -> Option<Rc<Worktree>> {
        self.active_slot().map(|slot| slot.worktree.clone())
    }

    /// アクティブプロジェクトの git 状態を読み直す（ツリー/タブの色分け）。git 無し/失敗は空。
    /// ディスク状態を反映するので、切替・オープン時に呼ぶ（編集中の未保存差分は gutter が担う）。
    /// git 状態（ツリー/タブ色・ブランチ・パネル用スナップショット）を**背景で**集めて反映する。
    /// 世代番号で古い結果を捨てる（gutter diff と同型）。UI スレッドで git を叩かない（ARCHITECTURE §9）。
    fn refresh_git_status(&mut self, cx: &mut Context<Self>) {
        self.refresh_git_status_for(self.project_sessions.active, cx);
    }

    fn refresh_git_status_for(&mut self, session_index: usize, cx: &mut Context<Self>) {
        let Some(worktree) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.worktree.clone())
        else {
            if let Some(session) = self.project_sessions.sessions.get_mut(session_index) {
                session.repository.status.clear();
                let git_panel = session.git_panel.clone();
                git_panel.update(cx, |panel, cx| panel.clear_snapshot(cx));
            }
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let Some(session) = self.project_sessions.sessions.get_mut(session_index) else {
            return;
        };
        session.repository.refresh_generation =
            session.repository.refresh_generation.wrapping_add(1);
        let generation = session.repository.refresh_generation;
        cx.spawn(async move |workspace, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move {
                    let status = project::git_status_on(host.as_ref(), &root);
                    let branch = project::git_current_branch_on(host.as_ref(), &root);
                    let changes = project::git_changes_on(host.as_ref(), &root);
                    let history = project::git_log_graph_on(host.as_ref(), &root, 30);
                    let slug = project::github_slug_on(host.as_ref(), &root);
                    (status, branch, changes, history, slug)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                let Some(session) = workspace.project_sessions.sessions.get_mut(session_index) else {
                    return;
                };
                if session.repository.refresh_generation != generation {
                    return; // 古い結果（その後に別の refresh が走った）
                }
                let (status, branch, changes, history, slug) = snapshot;
                session.repository.status = status.into_iter().collect();
                let git_panel = session.git_panel.clone();
                git_panel.update(cx, |panel, cx| {
                    panel.set_snapshot(
                        RepositorySnapshot { changes, history, github_slug: slug },
                        cx,
                    );
                });
                if let Some(slot) = workspace.project_sessions.projects.get_mut(session_index) {
                    slot.branch = branch;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// git 状態の色（UI-SPEC §1.3: 色は識別に集約。theme の診断/git トークンを流用）。
    fn git_tint(theme: &Theme, status: StatusKind) -> Hsla {
        match status {
            StatusKind::Untracked | StatusKind::Added => theme.ok, // 緑
            StatusKind::Modified => theme.warn,                    // 琥珀
            StatusKind::Deleted | StatusKind::Conflicted => theme.err, // 赤
        }
    }

    // git 状態の 1 文字バッジ（ツリー行末に出す）。
    fn git_letter(status: StatusKind) -> &'static str {
        match status {
            StatusKind::Untracked => "U",
            StatusKind::Added => "A",
            StatusKind::Modified => "M",
            StatusKind::Deleted => "D",
            StatusKind::Conflicted => "!",
        }
    }

    // ── branch / worktree メニュー（M8: ブランチ横断の完成形） ──

    // titlebar の ⎇ クリックで branch/worktree メニューを開閉する。
    // ⎇ メニューの開閉。ブランチ/worktree の列挙は背景で集め、揃ってから開く（UI は止めない）。
}
