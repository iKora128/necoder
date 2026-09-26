use crate::workspace::*;

mod auto_save;
mod diagnostics;
mod diff;
mod hot_exit;
mod inline_edit;
mod language;
mod overlays;
mod pins;
mod tabs;

/// 1 ProjectSession の編集面。tab / pane / language / diff / navigation の状態を一括所有する。
///
/// `EditorArea` 自身は長寿命 aggregate とし、実際に描画・入力を持つ各 `EditorView` を Entity として
/// 所有する。aggregate まで Entity で二重に包むと session 内の同期的な編集 command がすべて
/// cross-entity update になるため、ProjectSession からの委譲境界を意図的に残している。
pub struct EditorArea {
    pub(crate) loaded: bool,
    pub(crate) tabs: Vec<EditorTab>,
    pub(crate) active_tab: usize,
    pub(crate) split_editor: Option<Entity<EditorView>>,
    pub(crate) buffer_search: Option<BufferSearchState>,
    pub(crate) recently_closed_files: Vec<PathBuf>,
    pub(crate) lsp: Option<lang::lsp::LspClient>,
    pub(crate) lsp_root: Option<PathBuf>,
    pub(crate) lsp_language: Option<&'static str>,
    pub(crate) lsp_initialized: bool,
    pub(crate) lsp_incremental_sync: bool,
    pub(crate) lsp_sent_versions: HashMap<PathBuf, u64>,
    pub(crate) diagnostics: HashMap<PathBuf, Vec<(u32, lang::lsp::Severity)>>,
    pub(crate) raw_diagnostics: HashMap<PathBuf, serde_json::Value>,
    pub(crate) _lsp_pump: Option<gpui::Task<()>>,
    pub(crate) completion: Option<CompletionState>,
    pub(crate) completion_generation: u64,
    pub(crate) completion_suppressed_word: Option<usize>,
    pub(crate) hover: Option<HoverState>,
    pub(crate) hover_generation: u64,
    pub(crate) goto_line: Option<(String, FocusHandle)>,
    pub(crate) picker_symbol_rows: Vec<usize>,
    pub(crate) picker_workspace_symbols: Vec<(PathBuf, u32, u32)>,
    pub(crate) rename_input: Option<(String, FocusHandle)>,
    pub(crate) inline_edit: Option<InlineEditState>,
    pub(crate) code_actions: Option<CodeActionsState>,
    pub(crate) hunk_menu: Option<(project::DiffHunk, Point<gpui::Pixels>)>,
    pub(crate) pending_transient_tab: Option<(PathBuf, Buffer)>,
    pub(crate) pending_navigation: Option<(PathBuf, usize, usize)>,
    /// 開いた上で**整形プレビュー**を出す待ち行列（transcript の `.html` リンク・`▣ プレビュー`・
    /// Chat の成果物）。HTML か Markdown かは拡張子で決める。
    /// 行番号つきのジャンプは source を見たいので `pending_navigation` のまま。
    pub(crate) pending_preview: Option<PathBuf>,
    /// 上を開いた後、フォーカスを会話の入力欄へ戻すか（エージェントが書いた成果物を出す時）。
    pub(crate) pending_preview_keeps_focus: bool,
    /// 未保存の編集が無いタブを全部閉じる予約（Chat で見ているチャットが替わった時）。
    pub(crate) pending_close_clean_tabs: bool,
    pub(crate) pending_open_git_diff: Option<PathBuf>,
    /// Web タブで開く URL（`Workspace::open_url`・Window の無い入口から次の effect cycle へ渡す）。
    pub(crate) pending_web_url: Option<String>,
    pub(crate) pending_stage_hunk: Option<project::DiffHunk>,
    pub(crate) blame_gen: u32,
    pub(crate) last_blame_target: Option<(PathBuf, usize)>,
    pub(crate) nav_back: Vec<(PathBuf, usize)>,
    pub(crate) nav_forward: Vec<(PathBuf, usize)>,
    pub(crate) hot_exit_gen: u32,
    pub(crate) hot_exit_versions: HashMap<PathBuf, u64>,
    pub(crate) hot_exit_pending: Option<Vec<(PathBuf, String)>>,
}

impl EditorArea {
    pub(crate) fn new() -> Self {
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
            pending_preview: None,
            pending_preview_keeps_focus: false,
            pending_close_clean_tabs: false,
            pending_open_git_diff: None,
            pending_web_url: None,
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

impl DerefMut for ProjectSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.editor_area
    }
}
