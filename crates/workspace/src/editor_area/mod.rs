/// 1 ProjectSession の編集面。tab / pane / language / diff / navigation の状態を一括所有する。
///
/// 現段階では Workspace の既存 command routing を保つため ProjectSession から Deref する。
/// feature controller の移動後、この境界を child Entity の公開 API / typed event に置き換える。
pub struct EditorArea {
    loaded: bool,
    tabs: Vec<EditorTab>,
    active_tab: usize,
    split_editor: Option<Entity<EditorView>>,
    buffer_search: Option<BufferSearchState>,
    recently_closed_files: Vec<PathBuf>,
    lsp: Option<lang::lsp::LspClient>,
    lsp_root: Option<PathBuf>,
    lsp_language: Option<&'static str>,
    lsp_initialized: bool,
    lsp_incremental_sync: bool,
    lsp_sent_versions: HashMap<PathBuf, u64>,
    diagnostics: HashMap<PathBuf, Vec<(u32, lang::lsp::Severity)>>,
    raw_diagnostics: HashMap<PathBuf, serde_json::Value>,
    _lsp_pump: Option<gpui::Task<()>>,
    completion: Option<CompletionState>,
    completion_generation: u64,
    completion_suppressed_word: Option<usize>,
    hover: Option<HoverState>,
    hover_generation: u64,
    goto_line: Option<(String, FocusHandle)>,
    picker_symbol_rows: Vec<usize>,
    picker_workspace_symbols: Vec<(PathBuf, u32, u32)>,
    rename_input: Option<(String, FocusHandle)>,
    inline_edit: Option<InlineEditState>,
    code_actions: Option<CodeActionsState>,
    hunk_menu: Option<(project::DiffHunk, Point<gpui::Pixels>)>,
    pending_transient_tab: Option<(PathBuf, Buffer)>,
    pending_navigation: Option<(PathBuf, usize, usize)>,
    pending_open_git_diff: Option<PathBuf>,
    pending_stage_hunk: Option<project::DiffHunk>,
    blame_gen: u32,
    last_blame_target: Option<(PathBuf, usize)>,
    nav_back: Vec<(PathBuf, usize)>,
    nav_forward: Vec<(PathBuf, usize)>,
    hot_exit_gen: u32,
    hot_exit_versions: HashMap<PathBuf, u64>,
    hot_exit_pending: Option<Vec<(PathBuf, String)>>,
}

impl EditorArea {
    fn new() -> Self {
        Self {
            loaded: false,
            tabs: Vec::new(),
            active_tab: 0,
            split_editor: None,
            buffer_search: None,
            recently_closed_files: Vec::new(),
            lsp: None,
            lsp_root: None,
            lsp_language: None,
            lsp_initialized: false,
            lsp_incremental_sync: false,
            lsp_sent_versions: HashMap::new(),
            diagnostics: HashMap::new(),
            raw_diagnostics: HashMap::new(),
            _lsp_pump: None,
            completion: None,
            completion_generation: 0,
            completion_suppressed_word: None,
            hover: None,
            hover_generation: 0,
            goto_line: None,
            picker_symbol_rows: Vec::new(),
            picker_workspace_symbols: Vec::new(),
            rename_input: None,
            inline_edit: None,
            code_actions: None,
            hunk_menu: None,
            pending_transient_tab: None,
            pending_navigation: None,
            pending_open_git_diff: None,
            pending_stage_hunk: None,
            blame_gen: 0,
            last_blame_target: None,
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            hot_exit_gen: 0,
            hot_exit_versions: HashMap::new(),
            hot_exit_pending: None,
        }
    }
}

impl Deref for ProjectSession {
    type Target = EditorArea;

    fn deref(&self) -> &Self::Target {
        &self.editor_area
    }
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

impl DerefMut for ProjectSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.editor_area
    }
}
