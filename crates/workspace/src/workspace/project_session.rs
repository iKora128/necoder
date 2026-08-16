use crate::workspace::*;

/// Rail 上の project metadata と、遅延復元に必要なファイル一覧。
pub(crate) struct ProjectSlot {
    pub(crate) worktree: Rc<Worktree>,
    /// Fleet ではこの stable ID / lifecycle を参照し、rail の添字を identity にしない。
    pub(crate) task_space: TaskSpace,
    pub(crate) name: SharedString,
    /// 描画中に git process/RPC を起動しないための現在ブランチ cache。
    pub(crate) branch: Option<String>,
    /// Render が Host trait を呼ばず接続先を表示するための cache。local は None。
    pub(crate) remote_host: Option<SharedString>,
    pub(crate) color: Hsla,
    pub(crate) explorer: ExplorerProject,
    /// このプロジェクトで開いているタブのファイル一覧（左から順・M10 複数タブ）。
    /// アクティブ session の `EditorArea.tabs` から [`Workspace::sync_active_slot`] で同期する。
    pub(crate) open_files: Vec<PathBuf>,
    pub(crate) active_file: usize,
    /// `.shirushi/settings.json` の絵文字アイコン（None = 頭文字モノグラム）。
    pub(crate) icon: Option<SharedString>,
    /// リンク worktree ならブランチ名。通常/メイン slot は None。
    pub(crate) worktree_branch: Option<String>,
}

impl ProjectSlot {
    pub(crate) fn refresh(&mut self) {
        self.explorer.refresh(&self.worktree);
    }

    /// Explorer の Render はこのキャッシュだけを読み、FS/RPC を呼ばない。
    pub(crate) fn listed_dir(&self, dir: &Path) -> Vec<project::Entry> {
        self.explorer.listed_dir(dir)
    }
}

/// 「＋」統一オープン（[`PickerMode::OpenLauncher`]）の 1 行 → 確定時の動作。
/// id は [`ProjectSession::picker_open_rows`] の添字。固定アクション + 最近（local/remote 混在）。
#[derive(Clone)]
pub(crate) enum OpenRow {
    /// フォルダを開く（ネイティブのディレクトリダイアログ → 現レールに追加）。
    OpenFolder,
    /// ファイルを開く（ネイティブのファイルダイアログ → タブで開く）。
    OpenFile,
    /// リモートに接続（SSH ホストピッカーへ遷移）。
    ConnectRemote,
    /// 最近のローカルプロジェクト（このパスを現レールに開く）。
    RecentLocal(PathBuf),
    /// 最近のリモートプロジェクト（この `ssh://` URI へ接続して現レールに開く）。
    RecentRemote(String),
}

/// 1 project の長寿命 UI / controller 群。非アクティブ時も Entity と process を保持する。
pub struct ProjectSession {
    pub(crate) editor_area: EditorArea,
    pub(crate) agent_panel: Entity<AgentPanel>,
    pub(crate) explorer: Entity<Explorer>,
    pub(crate) search_panel: Option<Entity<SearchPanel>>,
    pub(crate) repository: RepositoryController,
    pub(crate) git_panel: Entity<GitPanel>,
    pub(crate) terminal_dock: Entity<TerminalDock>,
    /// Fleet の Tests surface 専用。通常 Terminal と同じ Entity を二箇所へ描画しない。
    pub(crate) tests_dock: Entity<TerminalDock>,
    pub(crate) agent_active: bool,
    pub(crate) picker_worktree_rows: Vec<PathBuf>,
    pub(crate) picker_ssh_hosts: Vec<host::SshConfigHost>,
    pub(crate) picker_ssh_recent: Vec<String>,
    /// スレッド履歴 Picker の行データ (id, name, color_index, created_at_ms, last_input_at_ms)。
    /// 時刻は復元時に Thread へ引き継ぐ（「いつスタート/最終入力」表示・M14）。
    pub(crate) picker_history: Vec<(String, String, i64, i64, Option<i64>)>,
    /// 「＋」統一オープンの行データ（id → 動作）。[`OpenRow`] を参照。
    pub(crate) picker_open_rows: Vec<OpenRow>,
    pub(crate) todo_panel: Entity<TodoPanel>,
    pub(crate) pending_open_history: bool,
    pub(crate) agent_touched: HashMap<PathBuf, Hsla>,
    pub(crate) waiting_thread: Option<(SharedString, Hsla)>,
    pub(crate) _watch: Option<project::Watch>,
    pub(crate) _watch_pump: Option<gpui::Task<()>>,
}

/// Rail metadata と長寿命 session を同じ添字で管理する非公開 owner。
pub(crate) struct ProjectSessions {
    pub(crate) projects: Vec<ProjectSlot>,
    pub(crate) active: usize,
    pub(crate) sessions: Vec<ProjectSession>,
}

pub(crate) struct RepositoryController {
    pub(crate) status: HashMap<PathBuf, StatusKind>,
    pub(crate) refresh_generation: u32,
}

impl Deref for ProjectSessions {
    type Target = ProjectSession;

    fn deref(&self) -> &Self::Target {
        &self.sessions[self.active]
    }
}

impl DerefMut for ProjectSessions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sessions[self.active]
    }
}

impl Workspace {
    pub(crate) fn create_project_session(
        slot: Option<&ProjectSlot>,
        theme: Theme,
        explorer_view: ExplorerView,
        storage: Option<storage::Storage>,
        cx: &mut Context<Self>,
    ) -> ProjectSession {
        let task_panel = slot.is_some_and(|slot| !slot.task_space.is_integration());
        let agent_panel = if task_panel {
            cx.new(|cx| AgentPanel::new_task(theme.clone(), cx))
        } else {
            cx.new(|cx| AgentPanel::new(theme.clone(), cx))
        };
        if let Some(storage) = storage {
            if let Some(slot) = slot {
                let scope = slot.task_space.id.0.clone();
                let legacy = slot.name.to_string();
                agent_panel.update(cx, |panel, cx| {
                    panel.set_storage_for_scope(storage, scope, &legacy, cx)
                });
            } else {
                agent_panel.update(cx, |panel, cx| panel.set_storage(storage, cx));
            }
        }
        let terminal_launch = Self::terminal_launch_for(slot);
        let terminal_dock = cx.new(|_| TerminalDock::new(terminal_launch, theme.clone()));
        let tests_dock =
            cx.new(|_| TerminalDock::new(Self::terminal_launch_for(slot), theme.clone()));
        let explorer = cx.new(|_| Explorer::new(explorer_view));
        let git_panel = cx.new(GitPanel::new);
        let accent = slot
            .map(|slot| slot.color)
            .unwrap_or_else(|| project_color(0));
        let todo_panel = cx.new(|_| TodoPanel::new(theme.clone(), accent));
        PanelRegistry::bind_session(
            &agent_panel,
            &explorer,
            &git_panel,
            &terminal_dock,
            &tests_dock,
            &todo_panel,
            cx,
        );
        ProjectSession {
            editor_area: EditorArea::new(),
            agent_panel,
            explorer,
            search_panel: None,
            repository: RepositoryController {
                status: HashMap::new(),
                refresh_generation: 0,
            },
            git_panel,
            terminal_dock,
            tests_dock,
            agent_active: false,
            picker_worktree_rows: Vec::new(),
            picker_ssh_hosts: Vec::new(),
            picker_ssh_recent: Vec::new(),
            picker_history: Vec::new(),
            picker_open_rows: Vec::new(),
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
    pub(crate) fn apply_remote_host_colors(&mut self) {
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
                    let index = key.bytes().fold(0u32, |acc, byte| {
                        acc.wrapping_mul(31).wrapping_add(byte as u32)
                    }) as usize
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

    /// DB の TaskSpace 台帳を、現在開いている worktree へ stable ID で重ねる。
    /// rail 順・active index・表示セルの有無には依存しない。未登録 worktree はこの場で台帳へ加える。
    pub(crate) fn restore_task_spaces(&mut self, cx: &mut Context<Self>) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        cx.spawn(async move |workspace, cx| {
            let storage_for_load = storage.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    let records = storage_for_load.load_task_spaces();
                    // ニュースの backfill（P2）: 直近イベントを新しい順で 60 件（描画スレッド外で読む）。
                    let events = storage_for_load
                        .load_recent_task_events(60)
                        .unwrap_or_default();
                    records.map(|records| (records, events))
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                let (records, events) = match loaded {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        eprintln!("TaskSpace 台帳を復元できない: {error:#}");
                        return;
                    }
                };
                let by_id: HashMap<_, _> = records
                    .into_iter()
                    .map(|record| (record.id.clone(), record))
                    .collect();
                let mut missing = Vec::new();
                for slot in &mut workspace.project_sessions.projects {
                    if let Some(record) = by_id.get(slot.task_space.id.as_str()) {
                        // lifecycle は台帳が正・kind は worktree の現実（branch 接頭辞）が正。
                        slot.task_space.repository_id = record.repository_id.clone();
                        slot.task_space.title = SharedString::from(record.title.clone());
                        slot.task_space.phase = record.phase;
                        slot.task_space.base_oid = record.base_oid.clone();
                        slot.task_space.head_oid = record.head_oid.clone();
                        slot.task_space.result_summary =
                            record.result_summary.clone().map(SharedString::from);
                        slot.task_space.created_at_ms = record.created_at;
                    } else {
                        missing.push(slot.task_space.to_record(slot));
                    }
                }
                if !missing.is_empty() {
                    let storage = storage.clone();
                    cx.background_executor()
                        .spawn(async move {
                            for record in missing {
                                if let Err(error) = storage.upsert_task_space(&record) {
                                    eprintln!("TaskSpace を永続化できない: {error:#}");
                                }
                            }
                        })
                        .detach();
                }
                // ニュースの backfill（P2）: live 行が既にあれば触らない（再起動直後だけ埋める）。
                // 帰属色は開いている slot から・閉じた Task は中立色（色=識別の規律: 不明に色を発明しない）。
                if workspace.notifications.news.is_empty() {
                    let neutral = workspace.theme.fg2;
                    let color_by_id: HashMap<String, Hsla> = workspace
                        .project_sessions
                        .projects
                        .iter()
                        .map(|slot| (slot.task_space.id.as_str().to_string(), slot.color))
                        .collect();
                    let mut backfill = Vec::new();
                    for event in &events {
                        let Some(record) = by_id.get(&event.task_id) else {
                            continue;
                        };
                        let payload: serde_json::Value =
                            serde_json::from_str(&event.payload).unwrap_or_default();
                        let digest = payload
                            .get("digest")
                            .and_then(serde_json::Value::as_str)
                            .or_else(|| payload.get("summary").and_then(serde_json::Value::as_str));
                        let (kind, text) = match event.kind.as_str() {
                            "phase_changed" => {
                                let phase = payload
                                    .get("phase")
                                    .and_then(serde_json::Value::as_str)
                                    .and_then(TaskPhase::from_str)
                                    .unwrap_or(record.phase);
                                Self::news_text_for_phase(phase, digest)
                            }
                            "task_created" => {
                                (NewsKind::PhaseChange, SharedString::from("→ planned"))
                            }
                            _ => continue,
                        };
                        backfill.push(NewsItem {
                            at_ms: event.created_at,
                            color: color_by_id.get(&event.task_id).copied().unwrap_or(neutral),
                            title: SharedString::from(record.title.clone()),
                            text,
                            kind,
                        });
                    }
                    workspace.notifications.news = backfill; // 既に新しい順（id DESC）
                }
                // 復元前に seed された index ベースの旧セルを残さない。
                if workspace.chrome.fleet_mode {
                    workspace.seed_fleet_cells(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn persist_task_space(&self, session_index: usize, cx: &mut Context<Self>) {
        let (Some(storage), Some(slot)) = (
            self.persistence.storage.clone(),
            self.project_sessions.projects.get(session_index),
        ) else {
            return;
        };
        let record = slot.task_space.to_record(slot);
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = storage.upsert_task_space(&record) {
                    eprintln!("TaskSpace を永続化できない: {error:#}");
                }
            })
            .detach();
    }

    /// Task lifecycle と event log を同時に進める。Agent runtime の状態とは別軸だが、
    /// permission wait / turn end の確定イベントを lifecycle へ写像する入口はここに集約する。
    /// `digest` = 遷移スナップショット（Tier 1・P1）。task_events の payload に載せて再起動後も残す。
    pub(crate) fn transition_task_space(
        &mut self,
        session_index: usize,
        phase: TaskPhase,
        reason: &str,
        digest: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get_mut(session_index) else {
            return;
        };
        if slot.task_space.is_integration() {
            return;
        }
        slot.task_space.phase = phase;
        slot.task_space.head_oid =
            project::git_head_oid_on(slot.worktree.host().as_ref(), slot.worktree.root());
        let record = slot.task_space.to_record(slot);
        let news_color = slot.color;
        let news_title = slot.task_space.title.clone();
        let payload = serde_json::json!({
            "phase": phase.as_str(),
            "source": "gui",
            "reason": reason,
            "digest": digest,
            "head_oid": record.head_oid,
        })
        .to_string();
        // ニュース = task_events の鏡（P2）。台帳へ書く遷移と同じ場所で 1 行積む。
        let (news_kind, news_text) = Self::news_text_for_phase(phase, digest);
        self.push_news(news_kind, news_color, news_title, news_text);
        // 監督バーの ✳ 総括はキューに影響する遷移からデバウンス生成（P4）。
        self.schedule_control_summary(cx);
        let Some(storage) = self.persistence.storage.clone() else {
            cx.notify();
            return;
        };
        cx.background_executor()
            .spawn(async move { storage.commit_task_transition(&record, &payload) })
            .detach();
        cx.notify();
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
                    let remote_host = worktree
                        .host()
                        .is_remote()
                        .then(|| SharedString::from(worktree.host().display_name().to_string()));
                    // `.shirushi/settings.json` の color(#hex)/icon(絵文字) を反映（M12-11）。
                    let identity = read_project_identity(worktree.root());
                    let mut slot = ProjectSlot {
                        task_space: TaskSpace::for_worktree(&worktree, None),
                        name: worktree.name().into(),
                        branch: None,
                        remote_host,
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
            let task_panel = projects
                .get(index)
                .is_some_and(|slot| !slot.task_space.is_integration());
            let agent_panel = if task_panel {
                cx.new(|cx| AgentPanel::new_task(theme.clone(), cx))
            } else {
                cx.new(|cx| AgentPanel::new(theme.clone(), cx))
            };
            let terminal_launch = Self::terminal_launch_for(projects.get(index));
            let terminal_dock = cx.new(|_| TerminalDock::new(terminal_launch, theme.clone()));
            let tests_dock = cx.new(|_| {
                TerminalDock::new(
                    Self::terminal_launch_for(projects.get(index)),
                    theme.clone(),
                )
            });
            let explorer = cx.new(|_| Explorer::new(explorer_view));
            let git_panel = cx.new(GitPanel::new);
            let accent = projects
                .get(index)
                .map(|slot| slot.color)
                .unwrap_or_else(|| project_color(0));
            let todo_panel = cx.new(|_| TodoPanel::new(theme.clone(), accent));
            PanelRegistry::bind_session(
                &agent_panel,
                &explorer,
                &git_panel,
                &terminal_dock,
                &tests_dock,
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
                repository: RepositoryController {
                    status: HashMap::new(),
                    refresh_generation: 0,
                },
                git_panel,
                terminal_dock,
                tests_dock,
                agent_active: false,
                picker_worktree_rows: Vec::new(),
                picker_ssh_hosts: Vec::new(),
                picker_ssh_recent: Vec::new(),
                picker_history: Vec::new(),
                picker_open_rows: Vec::new(),
                todo_panel,
                pending_open_history: false,
                agent_touched: HashMap::new(),
                waiting_thread: None,
                _watch: None,
                _watch_pump: None,
            });
        }
        let accent = projects
            .get(active)
            .map(|slot| slot.color)
            .unwrap_or_else(|| project_color(0));
        let settings_view = cx.new(|cx| settings::SettingsView::new(theme.clone(), accent, cx));
        PanelRegistry::bind_settings(&settings_view, cx);
        let fleet_mascot = cx.new(|_cx| agent_panel::MascotView::new(34.0));
        let mut workspace = Workspace {
            project_sessions: ProjectSessions {
                projects,
                active,
                sessions,
            },
            theme,
            focus_handle,
            chrome: ChromeState {
                show_left: true,
                show_right: true,
                show_bottom: false,
                // 編隊で起動したら左カラムの既定は herd（編隊の主役は Task 一覧）。
                show_herd: std::env::var_os("SHIRUSHI_HERD").is_some()
                    || std::env::var_os("SHIRUSHI_FLEET").is_some()
                    || std::env::var_os("SHIRUSHI_CONTROL").is_some(),
                fleet_mode: std::env::var_os("SHIRUSHI_FLEET").is_some()
                    || std::env::var_os("SHIRUSHI_CONTROL").is_some(),
                fleet_cells: Vec::new(),
                fleet_seeded: false,
                fleet_cell_menu: None,
                fleet_bottom_view: FleetBottomView::News,
                agent_full_screen: std::env::var_os("SHIRUSHI_AGENT_FULLSCREEN").is_some(),
                bottom_height: BOTTOM_DOCK_HEIGHT,
                resizing_bottom: false,
                resize_start_y: 0.0,
                resize_start_height: 0.0,
                // 管制タブ（P3）。既定は当面 Graph（計画 §P3・ドッグフーディング後に再判断）。
                fleet_center_view: if std::env::var_os("SHIRUSHI_CONTROL").is_some() {
                    FleetCenterView::Control
                } else {
                    FleetCenterView::Graph
                },
                graph_view: match std::env::var("SHIRUSHI_GRAPH").as_deref() {
                    Ok("hub") => GraphView::Hub,
                    Ok("tree") => GraphView::Tree,
                    Ok("card") => GraphView::Card,
                    _ => GraphView::Fan,
                },
                graph_collapsed: false,
                fleet_maximized: std::env::var("SHIRUSHI_FLEET_MAX")
                    .ok()
                    .and_then(|value| value.parse().ok()),
                fleet_clock: false,
                rollup_index: 0,
                rollup_ticker: false,
                show_settings: std::env::var_os("SHIRUSHI_SETTINGS").is_some()
                    || (!settings::get(cx).onboarded
                        && !(cfg!(debug_assertions)
                            && std::env::var_os("SHIRUSHI_SCREENSHOT").is_some())),
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
                rail_drag: None,
                control_focus: cx.focus_handle(),
                herd_solo_expanded: false,
                herd_renaming: None,
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
                worktree_delete: None,
                ssh_input: None,
                ssh_connecting: false,
                add_project_dialog_open: false,
                pending_project_switch: None,
            },
            notifications: NotificationCenter {
                toasts: Vec::new(),
                toast_gen: 0,
                crash_notice: None,
                news: Vec::new(),
            },
            persistence: WorkspacePersistence {
                state_path,
                storage: None,
            },
            updater: UpdateController { status: None },
            fleet_mascot,
            window_active: true,
            visual_tick: 0,
            visual_ticker: false,
            control_summary: None,
            control_summary_gen: 0,
        };
        workspace.refresh_all_git_status(cx); // ツリー/タブの git 色分け + herd の各 space のブランチ（M14 ①）
                                              // 開発用: SHIRUSHI_GIT_PANEL=1 で git 操作パネル（ソース管理）を開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_GIT_PANEL").is_some() {
            workspace
                .git_panel
                .update(cx, |panel, cx| panel.set_open(true, cx));
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
            let hex = if probe == "1" {
                String::new()
            } else {
                probe.trim_start_matches('#').to_string()
            };
            workspace.overlays.color_picker = Some(ColorPickerState {
                project_index: workspace.project_sessions.active,
                position: point(px(RAIL_WIDTH), px(12.)),
                hex,
                focus: cx.focus_handle(),
            });
        }
        // 開発用: SHIRUSHI_RAIL_MENU=1 でレール項目の右クリックメニューを開いた状態で撮る（M10-2）。
        // worktree/ブランチ削除行も見せるため、アクティブスロットを worktree タブ扱いにする。
        // （二段確認は 2026-07-27 に確認ダイアログへ一本化＝`SHIRUSHI_WORKTREE_DELETE_PROBE` を使う）
        if std::env::var_os("SHIRUSHI_RAIL_MENU").is_some() {
            let active = workspace.project_sessions.active;
            if let Some(slot) = workspace.project_sessions.projects.get_mut(active) {
                slot.worktree_branch = Some("feature/login".to_string());
            }
            workspace.overlays.rail_menu = Some(RailMenuState {
                project_index: workspace.project_sessions.active,
                position: point(px(RAIL_WIDTH), px(12.)),
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
            if let Some(root) = workspace
                .active_worktree()
                .map(|worktree| worktree.root().to_path_buf())
            {
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
        // Fleet は全 Task の composer を同時に操作できるので、active だけでなく全 session の宛先を設定。
        for index in 0..workspace.project_sessions.projects.len() {
            workspace.update_agent_destination_for(index, cx);
        }
        workspace.start_watcher(cx); // アクティブプロジェクトのファイル監視（M10 watch 基盤）
        workspace.loaded = true;
        workspace.schedule_update_check(cx); // 自動アップデートの確認（M13・90s 後に背景で）
        workspace.check_crash_notice(cx); // 前回クラッシュの通知（M13・pending マーカーを 1 回だけ消費）
                                          // 開発用: SHIRUSHI_UPDATE_PROBE="x.y.z" でチップ描画を直接確認（ネット不要）。
        if let Ok(version) = std::env::var("SHIRUSHI_UPDATE_PROBE") {
            if !version.is_empty() {
                workspace.updater.status = Some((
                    updater::UpdateInfo {
                        version,
                        dmg_url: String::new(),
                    },
                    UpdateState::Available,
                ));
            }
        }
        // ローカル永続化 DB（hot exit・M10）。SHIRUSHI_DB でパス上書き（検証用）。開けなくても起動は続行。
        let db_path = std::env::var("SHIRUSHI_DB")
            .map(PathBuf::from)
            .ok()
            .or_else(|| (!cfg!(test)).then(storage::default_db_path).flatten());
        if let Some(db_path) = db_path {
            match storage::Storage::open(&db_path) {
                Ok(handle) => {
                    // Agent パネルへも同じハンドルを渡す（スレッド永続化・M12-1。ワーカー 1 本を共有）。
                    for (index, session) in workspace.project_sessions.sessions.iter().enumerate() {
                        if let Some(slot) = workspace.project_sessions.projects.get(index) {
                            let scope = slot.task_space.id.0.clone();
                            let legacy = slot.name.to_string();
                            session.agent_panel.update(cx, |panel, cx| {
                                panel.set_storage_for_scope(handle.clone(), scope, &legacy, cx)
                            });
                        } else {
                            session
                                .agent_panel
                                .update(cx, |panel, cx| panel.set_storage(handle.clone(), cx));
                        }
                    }
                    workspace.persistence.storage = Some(handle);
                    // リモートプロジェクトの窓色をローカル DB から解決（M13 #3b）。
                    workspace.apply_remote_host_colors();
                    workspace.restore_task_spaces(cx);
                    if let Some(storage) = &workspace.persistence.storage {
                        for slot in &workspace.project_sessions.projects {
                            if !slot.worktree.host().is_remote() {
                                let _ = storage.record_local_project(
                                    &slot.worktree.root().to_string_lossy(),
                                    slot.name.as_ref(),
                                );
                            }
                        }
                    }
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
    pub fn restore_open_file(
        &mut self,
        restored: &[RestoredTabs],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(crate) fn active_slot(&self) -> Option<&ProjectSlot> {
        self.project_sessions
            .projects
            .get(self.project_sessions.active)
    }

    /// 現在アクティブなタブのエディタ（無ければ `None`）。従来 `self.editor` を読んでいた箇所の置換。
    pub(crate) fn active_editor(&self) -> Option<Entity<EditorView>> {
        self.tabs.get(self.active_tab).map(|tab| tab.editor.clone())
    }

    /// アクティブタブのファイルパス。
    pub(crate) fn active_tab_path(&self) -> Option<PathBuf> {
        self.tabs.get(self.active_tab).map(|tab| tab.path.clone())
    }

    /// 現在のタブ列をアクティブ slot へ書き戻す（永続化・切替復元の真実源を同期）。
    pub(crate) fn sync_active_slot(&mut self) {
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
    pub(crate) fn open_transient_tab(
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
        let accent = self
            .active_slot()
            .map(|slot| slot.color)
            .unwrap_or_else(|| project_color(0));
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
    pub(crate) fn open_slot_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // タブ列ごと入れ替わる（レール/ブランチ切替）ので ⌘F バー・hover は畳む。
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let (files, active_file) = match self.active_slot() {
            Some(slot) => (slot.open_files.clone(), slot.active_file),
            None => return,
        };
        let host = self
            .active_worktree()
            .map(|worktree| worktree.host().clone());
        self.tabs.clear();
        self.active_tab = 0;
        for path in files {
            let exists = host
                .as_ref()
                .map(|host| host.metadata(&path).is_ok())
                .unwrap_or(false);
            if exists {
                // 背景読み込みだと完了順でタブ順が崩れるため、復元だけは同期で開く。
                self.open_file_sync(path, window, cx);
            }
        }
        if active_file < self.tabs.len() {
            self.select_tab(active_file, window, cx);
        }
    }

    pub(crate) fn active_worktree(&self) -> Option<Rc<Worktree>> {
        self.active_slot().map(|slot| slot.worktree.clone())
    }

    /// アクティブプロジェクトの git 状態を読み直す（ツリー/タブの色分け）。git 無し/失敗は空。
    /// ディスク状態を反映するので、切替・オープン時に呼ぶ（編集中の未保存差分は gutter が担う）。
    /// git 状態（ツリー/タブ色・ブランチ・パネル用スナップショット）を**背景で**集めて反映する。
    /// 世代番号で古い結果を捨てる（gutter diff と同型）。UI スレッドで git を叩かない（ARCHITECTURE §9）。
    pub(crate) fn refresh_git_status(&mut self, cx: &mut Context<Self>) {
        self.refresh_git_status_for(self.project_sessions.active, cx);
    }

    /// 全 slot の git 状態（＝各 worktree space の**現在ブランチ**）を背景で埋める。herd/⌘O が
    /// 非アクティブ space のブランチも出せるようにする（`refresh_git_status` は active のみ・M14 ①）。
    /// 起動時とレール増減時に回す。各 slot 独立の背景 spawn（世代番号で古い結果は破棄）。
    pub(crate) fn refresh_all_git_status(&mut self, cx: &mut Context<Self>) {
        for index in 0..self.project_sessions.sessions.len() {
            self.refresh_git_status_for(index, cx);
        }
    }

    pub(crate) fn refresh_git_status_for(&mut self, session_index: usize, cx: &mut Context<Self>) {
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
                    // linked worktree か（＝削除できる作業ツリーか）を **git に聞く**（2026-07-27）。
                    // 以前は「このセッションで worktree として開いたか」の記憶だけが根拠だったので、
                    // 再起動すると worktree なのに削除メニューが消え、消す手段が無くなっていた。
                    // 一覧の先頭 = メイン作業ツリー。それ以外に自分が居れば linked。
                    let linked = {
                        let worktrees = project::git_worktrees_on(host.as_ref(), &root);
                        let canonical =
                            std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
                        worktrees
                            .iter()
                            .skip(1)
                            .find(|worktree| {
                                let path = std::fs::canonicalize(&worktree.path)
                                    .unwrap_or_else(|_| worktree.path.clone());
                                path == canonical
                            })
                            .map(|worktree| worktree.branch.clone())
                    };
                    (status, branch, changes, history, slug, linked)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                let Some(session) = workspace.project_sessions.sessions.get_mut(session_index)
                else {
                    return;
                };
                if session.repository.refresh_generation != generation {
                    return; // 古い結果（その後に別の refresh が走った）
                }
                let (status, branch, changes, history, slug, linked) = snapshot;
                session.repository.status = status.into_iter().collect();
                let git_panel = session.git_panel.clone();
                git_panel.update(cx, |panel, cx| {
                    panel.set_snapshot(
                        RepositorySnapshot {
                            changes,
                            history,
                            github_slug: slug,
                        },
                        cx,
                    );
                });
                if let Some(slot) = workspace.project_sessions.projects.get_mut(session_index) {
                    slot.branch = branch.clone();
                    // git が「linked worktree だ」と言うなら常にそれが正（起動経路に依存しない）。
                    // メイン作業ツリーだった場合は None に戻す＝main を消せる導線を作らない。
                    slot.worktree_branch =
                        linked.map(|linked| linked.or(branch).unwrap_or_default());
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// git 状態の色（UI-SPEC §1.3: 色は識別に集約。theme の診断/git トークンを流用）。
    pub(crate) fn git_tint(theme: &Theme, status: StatusKind) -> Hsla {
        match status {
            StatusKind::Untracked | StatusKind::Added => theme.ok, // 緑
            StatusKind::Modified => theme.warn,                    // 琥珀
            StatusKind::Deleted | StatusKind::Conflicted => theme.err, // 赤
        }
    }

    // git 状態の 1 文字バッジ（ツリー行末に出す）。
    pub(crate) fn git_letter(status: StatusKind) -> &'static str {
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

impl ProjectSessions {
    /// アクティブ session の明示アクセサ（Deref の暗黙形に対する明示形）。
    pub(crate) fn active_session(&self) -> &ProjectSession {
        &self.sessions[self.active]
    }

    pub(crate) fn active_session_mut(&mut self) -> &mut ProjectSession {
        &mut self.sessions[self.active]
    }
}

impl Workspace {
    /// アクティブ session（明示形）。**新規コードは Deref 経由の `self.<session フィールド>` でなく
    /// こちらを使う**（アクセス時点の active に依存することをコード上で見えるようにする・§7.5-5）。
    pub(crate) fn session(&self) -> &ProjectSession {
        self.project_sessions.active_session()
    }

    pub(crate) fn session_mut(&mut self) -> &mut ProjectSession {
        self.project_sessions.active_session_mut()
    }
}
