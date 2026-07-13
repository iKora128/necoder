//! workspace — レール（プロジェクト切替）+ 左ドックのエクスプローラ + 中央エディタ + ステータスバー。
//!
//! ARCHITECTURE §5: 1 窓 = アクティブな (project, branch)。レール = 窓内切替。UI-SPEC §1.3 の色の
//! 許可リストに従い、**プロジェクト色**はレール枠/リング・ツリー選択の左バー・キャレットにのみ流す。
//! 状態（開プロジェクト・アクティブ・開ファイル）は `state.json` に保存し、再起動で復元する。

use agent_panel::AgentPanel;
use editor_core::Buffer;
use futures::StreamExt as _; // LSP 通知 pump の `.next()`
use editor_view::EditorView;
use gpui::{
    Animation, AnimationExt, App, Bounds, ClipboardItem, Context, CursorStyle, Entity, FocusHandle,
    Focusable, FontWeight, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Point, SharedString, Subscription, TitlebarOptions, Window,
    WindowBounds, WindowControlArea, WindowOptions, actions, div, point, prelude::*,
    pulsating_between, px, size,
};
use host::Host;
use project::{GitWorktree, GraphCommit, StatusKind, WorkingChange, Worktree};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use terminal_view::TerminalView;
use theme_core::{Theme, ThemeSource, project_color};
use ui::Tooltip;
use ui::{DraggedFile, Picker, PickerEvent, PickerItem};

actions!(
    workspace,
    [
        FileFinder,
        ProjectSwitcher,
        ProjectSearch,
        ThemeSelector,
        ToggleTerminal,
        GoToDefinition,
        TriggerCompletion,
        SplitRight,
        ToggleGitPanel,
        CloseTab,
        NewThread,
        // AI スレッドタブの切替（Chrome 風。⌘⌥←→ / ⌃Tab）。
        SelectNextThread,
        SelectPrevThread,
        // アクティブ (project, branch) を新しいウィンドウで開く（⌘⇧N。ウィンドウモデル §5）。
        // 当初 ROADMAP は ⌘⏎ を想定したが、composer の agent::SubmitPrompt と衝突するため
        // 慣例的な ⌘⇧N に変更（Editor コンテキストの ⌘⏎ は送信のまま温存）。
        NewWindow,
        // レール上のプロジェクト N 番へ切替（⌘1..9）。窓内切替＝ウィンドウモデル §5。
        ActivateProject1,
        ActivateProject2,
        ActivateProject3,
        ActivateProject4,
        ActivateProject5,
        ActivateProject6,
        ActivateProject7,
        ActivateProject8,
        ActivateProject9,
    ]
);

#[derive(Clone, Copy, PartialEq)]
enum PickerMode {
    Files,
    Projects,
    Themes,
}

const RAIL_WIDTH: f32 = 46.0;
const DOCK_WIDTH: f32 = 218.0;
const DOCK_MIN: f32 = 150.0;
const DOCK_MAX: f32 = 640.0;
const ROW_HEIGHT: f32 = 23.0;
const INDENT: f32 = 14.0;
const AGENT_DOCK_WIDTH: f32 = 440.0; // Agent パネル既定幅（ドラッグで可変）
const AGENT_DOCK_MIN: f32 = 320.0;
const AGENT_DOCK_MAX: f32 = 900.0;
const RESIZE_HANDLE_WIDTH: f32 = 6.0;
const TITLEBAR_HEIGHT: f32 = 38.0;
const TABSTRIP_HEIGHT: f32 = 34.0;
const BREADCRUMB_HEIGHT: f32 = 26.0;
const STATUSBAR_HEIGHT: f32 = 26.0;
const BOTTOM_DOCK_HEIGHT: f32 = 240.0; // 下ドック（ターミナル）の高さ
/// macOS のネイティブ信号機（appears_transparent 時も残る）を避けるための左余白。
const TRAFFIC_LIGHT_INSET: f32 = 78.0;

/// titlebar / statusbar のドックトグルが指すドック。
#[derive(Clone, Copy, PartialEq)]
enum Dock {
    Left,
    Right,
    Bottom,
}

// ── 状態永続化 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedProject {
    root: PathBuf,
    #[serde(default)]
    open_file: Option<PathBuf>,
    /// `None` は従来どおり local。password を含まない正規 `ssh://` URI のみ保存する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remote_uri: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedState {
    projects: Vec<PersistedProject>,
    #[serde(default)]
    active: usize,
}

/// `state.json` の標準パス（macOS）。
pub fn state_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/Shirushi/state.json"))
}

/// 前回の (プロジェクト群, アクティブ) を読む。無ければ `None`。
pub fn load_state(path: &Path) -> Option<(Vec<PathBuf>, usize)> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: PersistedState = serde_json::from_str(&text).ok()?;
    if state.projects.is_empty() {
        return None;
    }
    let roots = state.projects.into_iter().map(|project| project.root).collect();
    Some((roots, state.active))
}

/// 起動前の接続解決に使う、永続化済み project 記述。
#[derive(Debug, Clone)]
pub struct SavedProject {
    pub root: PathBuf,
    pub open_file: Option<PathBuf>,
    pub remote_uri: Option<String>,
}

/// local/SSH を区別して前回状態を読む。旧 `root` だけの state.json と後方互換。
pub fn load_saved_state(path: &Path) -> Option<(Vec<SavedProject>, usize)> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: PersistedState = serde_json::from_str(&text).ok()?;
    if state.projects.is_empty() {
        return None;
    }
    let projects = state
        .projects
        .into_iter()
        .map(|project| SavedProject {
            root: project.root,
            open_file: project.open_file,
            remote_uri: project.remote_uri,
        })
        .collect();
    Some((projects, state.active))
}

/// 1 project の実行先。`PathBuf` 単体に戻すと異なる host の同一パスが衝突するため常に組で扱う。
#[derive(Clone)]
pub struct ProjectSource {
    host: Arc<dyn Host>,
    root: PathBuf,
}

impl ProjectSource {
    pub fn local(root: PathBuf) -> Self {
        Self { host: host::LocalHost::shared(), root }
    }

    pub fn new(host: Arc<dyn Host>, root: PathBuf) -> Self {
        Self { host, root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_remote()
    }
}

// ── ツリー ──

struct TreeRow {
    path: PathBuf,
    name: SharedString,
    is_dir: bool,
    depth: usize,
    is_expanded: bool,
    /// gitignore 対象（薄字で描く）。
    ignored: bool,
}

fn build_rows(
    worktree: &Worktree,
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    rows: &mut Vec<TreeRow>,
) {
    let Ok(entries) = worktree.read_dir(dir) else {
        return;
    };
    for entry in entries {
        let is_expanded = entry.is_dir && expanded.contains(&entry.path);
        rows.push(TreeRow {
            path: entry.path.clone(),
            name: entry.name.into(),
            is_dir: entry.is_dir,
            depth,
            is_expanded,
            ignored: entry.ignored,
        });
        if is_expanded {
            build_rows(worktree, &entry.path, depth + 1, expanded, rows);
        }
    }
}

/// エクスプローラの右クリックメニュー（対象パス + 種別 + 出す位置）。
struct ExplorerContextMenu {
    path: PathBuf,
    is_dir: bool,
    position: Point<gpui::Pixels>,
}

/// エクスプローラの表示モード（Finder 風の 3 種。左下で切替）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExplorerView {
    /// 縦ツリー（従来）。
    Tree,
    /// Finder のカラム（Miller columns）。
    Columns,
    /// アイコングリッド。
    Icons,
}

struct ProjectSlot {
    worktree: Rc<Worktree>,
    name: SharedString,
    /// 描画中に git process/RPC を起動しないための現在ブランチ cache。
    branch: Option<String>,
    color: Hsla,
    expanded: HashSet<PathBuf>,
    rows: Vec<TreeRow>,
    selected: Option<PathBuf>,
    open_file: Option<PathBuf>,
    /// カラム/アイコン表示の「現在フォルダ」（ここの直下を見せる）。既定はルート。
    current_dir: Option<PathBuf>,
}

struct BranchMenuState {
    position: Point<gpui::Pixels>,
    current: Option<String>,
    branches: Vec<String>,
    worktrees: Vec<GitWorktree>,
}

impl ProjectSlot {
    fn refresh(&mut self) {
        let mut rows = Vec::new();
        let root = self.worktree.root().to_path_buf();
        build_rows(&self.worktree, &root, 0, &self.expanded, &mut rows);
        self.rows = rows;
    }
}

// ── プロジェクト横断検索パネル（M6） ──

/// 横断検索の走査対象ファイル上限（ファイルファインダと同じ 5000）。
const SEARCH_FILE_LIMIT: usize = 5000;
/// 結果パネルに描くマッチ行の上限（描画コストを抑える）。
const SEARCH_MAX_ROWS: usize = 300;

/// プロジェクト横断検索パネルの状態（オーバーレイ）。クエリ + 大小/正規表現トグル + ファイル別結果。
struct SearchState {
    query: String,
    case_sensitive: bool,
    is_regex: bool,
    results: Vec<search::FileMatch>,
    /// 現在の `results` を生んだクエリ。Enter を「検索実行」か「選択へジャンプ」に振り分ける。
    results_query: Option<String>,
    /// 平坦化した選択位置（結果全体での通し番号）。矢印キーで移動。
    selected: usize,
    /// 直近クエリのコンパイルエラー（正規表現の誤り等）。
    error: Option<SharedString>,
    running: bool,
    active_search: Option<u64>,
    focus: FocusHandle,
}

impl SearchState {
    /// `(file_index, match_index)` の平坦列（キーボード選択と描画行の対応）。
    fn flat(&self) -> Vec<(usize, usize)> {
        let mut flat = Vec::new();
        for (file_index, file) in self.results.iter().enumerate() {
            for match_index in 0..file.matches.len() {
                flat.push((file_index, match_index));
            }
        }
        flat
    }

    /// マッチ総数（ヘッダ表示用）。
    fn total_matches(&self) -> usize {
        self.results.iter().map(|file| file.matches.len()).sum()
    }
}

// ── LSP 補完ポップアップ（M7） ──

/// 補完の 1 候補。
struct CompletionItem {
    label: SharedString,
    /// 挿入テキスト（textEdit.newText → insertText → label）。
    insert_text: String,
    detail: Option<SharedString>,
    /// 種別の短い記号（fn/struct/let 等）。
    kind: SharedString,
}

/// 補完ポップアップ（オーバーレイ）。エディタのキャレット直下に出す。フォーカスを取り上下/確定/中止を受ける。
struct CompletionState {
    items: Vec<CompletionItem>,
    selected: usize,
    position: Point<gpui::Pixels>,
    focus: FocusHandle,
}

/// LSP の CompletionItemKind を短い記号へ（表示用）。
fn completion_kind_label(kind: Option<u64>) -> &'static str {
    // https://microsoft.github.io/language-server-protocol/specifications/specification-current/#completionItemKind
    match kind {
        Some(2) | Some(3) => "fn",   // Method / Function
        Some(5) => "field",          // Field
        Some(6) => "let",            // Variable
        Some(7) | Some(22) => "type", // Class / Struct
        Some(8) => "trait",          // Interface
        Some(9) => "mod",            // Module
        Some(14) => "kw",            // Keyword
        Some(21) => "const",         // Constant
        _ => "•",
    }
}

/// LSP の completion 結果（Array or List）を候補列へ。
fn parse_completion_items(value: &serde_json::Value) -> Vec<CompletionItem> {
    let array = if value.is_array() {
        value.as_array()
    } else {
        value.get("items").and_then(|items| items.as_array())
    };
    let Some(array) = array else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|item| {
            let label = item.get("label")?.as_str()?.to_string();
            // 挿入テキスト: textEdit.newText → insertText → label。
            let insert_text = item
                .get("textEdit")
                .and_then(|edit| edit.get("newText"))
                .and_then(|text| text.as_str())
                .or_else(|| item.get("insertText").and_then(|text| text.as_str()))
                .unwrap_or(&label)
                .to_string();
            let detail = item
                .get("detail")
                .and_then(|detail| detail.as_str())
                .map(|detail| SharedString::from(detail.to_string()));
            let kind = SharedString::from(completion_kind_label(
                item.get("kind").and_then(serde_json::Value::as_u64),
            ));
            Some(CompletionItem { label: SharedString::from(label), insert_text, detail, kind })
        })
        .take(60)
        .collect()
}

/// 定義ジャンプの結果（Location / Location[] / LocationLink[]）から (パス, 行, character) を取る。
fn parse_definition(value: &serde_json::Value) -> Option<(PathBuf, u32, u32)> {
    let location = if value.is_array() {
        value.as_array()?.first()?
    } else if value.is_object() {
        value
    } else {
        return None;
    };
    // LocationLink（targetUri）優先、無ければ Location（uri）。
    let (uri, range) = if let Some(uri) = location.get("targetUri").and_then(|uri| uri.as_str()) {
        let range = location
            .get("targetSelectionRange")
            .or_else(|| location.get("targetRange"))?;
        (uri, range)
    } else {
        let uri = location.get("uri")?.as_str()?;
        let range = location.get("range")?;
        (uri, range)
    };
    let path = lang::lsp::uri_to_path(uri)?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as u32;
    let character = start.get("character")?.as_u64()? as u32;
    Some((path, line, character))
}

// ── Workspace 本体 ──

pub struct Workspace {
    projects: Vec<ProjectSlot>,
    active: usize,
    editor: Option<Entity<EditorView>>,
    // 右分割ペイン（⌘\ で開閉。独立エディタ＝比較・参照用の副ビュー。LSP/統合は主ペイン=editor 側）。
    split_editor: Option<Entity<EditorView>>,
    agent_panel: Entity<AgentPanel>,
    theme: Theme,
    focus_handle: FocusHandle,
    state_path: Option<PathBuf>,
    // ドックの表示状態（titlebar のアイコンでトグル）。右=Agent パネル・下=ターミナルは M4/M8。
    show_left: bool,
    show_right: bool,
    show_bottom: bool,
    // Agent パネルの可変幅（左縁ドラッグで調整）。
    agent_width: f32,
    resizing_agent: bool,
    resize_start_x: f32,
    resize_start_width: f32,
    // 左ドック（エクスプローラ）の可変幅（右縁ドラッグで調整）。
    explorer_width: f32,
    resizing_explorer: bool,
    // titlebar ドラッグ（down→move で窓移動開始。クリック/ダブルクリックと区別する）。
    should_move_window: bool,
    _editor_observation: Option<Subscription>,
    // オーバーレイ（ファイルファインダ / プロジェクトスイッチャー）
    picker: Option<Entity<Picker>>,
    picker_mode: PickerMode,
    picker_files: Vec<PathBuf>,
    // テーマセレクタの候補（表示名 + 出所）。Picker の id はこの Vec の添字。
    picker_themes: Vec<(SharedString, ThemeSource)>,
    // ライブプレビュー前のテーマ（Dismiss で戻す）。
    theme_before_preview: Option<Theme>,
    _picker_observation: Option<Subscription>,
    // エクスプローラの表示モード（ツリー/カラム/アイコン）。左下で切替。
    explorer_view: ExplorerView,
    // エクスプローラの右クリックメニュー（開いていれば Some）。
    explorer_context_menu: Option<ExplorerContextMenu>,
    // プロジェクト横断検索パネル（開いていれば Some・⌘⇧F）。
    search_panel: Option<SearchState>,
    next_search_id: u64,
    // アクティブプロジェクトの git 状態（絶対パス → 状態）。ツリー/タブの色分けに使う。
    git_status: HashMap<PathBuf, StatusKind>,
    // Git パネルを描画するたびに process/RPC を起動しないための snapshot。
    git_changes: Vec<WorkingChange>,
    git_history: Vec<GraphCommit>,
    // origin が GitHub なら owner/repo（PR ボタンの表示判定。M8: GitHub 連携）。
    github_slug: Option<String>,
    // branch/worktree メニュー（titlebar の ⎇ クリックで開く。開いていれば出す位置）。
    branch_menu: Option<BranchMenuState>,
    // 下ドックの統合ターミナル（初回表示で遅延生成。show_bottom で出す）。
    terminal: Option<Entity<TerminalView>>,
    // ── LSP（言語サーバ・M7。拡張子→サーバの登録式で多言語対応）──
    lsp: Option<lang::lsp::LspClient>,
    lsp_root: Option<PathBuf>,
    // 現在稼働中のサーバが担当する languageId（"rust"/"typescript"/… ）。ファイル言語が
    // 変わったら張り替える判定に使う。
    lsp_language: Option<&'static str>,
    lsp_initialized: bool,
    // 最後に didChange で送ったバッファ version（重複送信の抑止）。
    lsp_sent_version: u64,
    // ファイル別診断（絶対パス → (行, 重大度)）。gutter/statusbar に使う。
    diagnostics: HashMap<PathBuf, Vec<(u32, lang::lsp::Severity)>>,
    _lsp_pump: Option<gpui::Task<()>>,
    // 補完ポップアップ（開いていれば。Ctrl-Space で開く）。
    completion: Option<CompletionState>,
    // ── git 操作パネル（M8: ソース管理。⌃⇧G で左カラムをエクスプローラと切替）──
    git_panel: Option<GitPanelState>,
    // push/pull はネットワークで遅い → 背景実行中は true（ボタン無効化・表示用）。
    git_busy: bool,
}

/// git 操作パネルの状態（コミットメッセージ / ブランチ名の手書き入力を持つ）。
/// 入力は検索パネルと同じ流儀（keystroke を直接 String に積む）。
struct GitPanelState {
    /// コミットメッセージの編集バッファ。
    message: String,
    /// `Some` のときは入力行が「新しいブランチ名」モード（enter で作成）。
    branch_name: Option<String>,
    /// キーストローク取り込み用のフォーカス。
    focus: FocusHandle,
}

impl Workspace {
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
    pub fn new_sources_with_active(
        sources: Vec<ProjectSource>,
        active: usize,
        theme: Theme,
        state_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut projects = Vec::new();
        for source in sources {
            match Worktree::with_host(source.host, &source.root) {
                Ok(worktree) => {
                    let index = projects.len();
                    let mut slot = ProjectSlot {
                        name: worktree.name().into(),
                        branch: None,
                        color: project_color(index),
                        worktree: Rc::new(worktree),
                        expanded: HashSet::new(),
                        rows: Vec::new(),
                        selected: None,
                        open_file: None,
                        current_dir: None,
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
                slot.current_dir = slot.worktree.root().parent().map(Path::to_path_buf);
            }
        }
        // 開発用フックの値は projects が move される前に計算しておく。
        let explorer_context_menu = std::env::var_os("SHIRUSHI_CONTEXT_MENU").and_then(|_| {
            projects.first().map(|slot| ExplorerContextMenu {
                path: slot.worktree.root().to_path_buf(),
                is_dir: true,
                position: point(px(120.0), px(210.0)),
            })
        });
        // 開発用: SHIRUSHI_SEARCH_PANEL=1 で「fn」を横断検索した結果パネルを開いた状態で撮る。
        let search_panel = std::env::var_os("SHIRUSHI_SEARCH_PANEL").and_then(|_| {
            projects.first().map(|slot| {
                let results = search::SearchQuery::new("fn", false, false)
                    .map(|query| {
                        query.search_project_on(
                            slot.worktree.host().as_ref(),
                            slot.worktree.root(),
                            SEARCH_FILE_LIMIT,
                            SEARCH_MAX_ROWS,
                        )
                    })
                    .unwrap_or_default();
                SearchState {
                    query: "fn".to_string(),
                    case_sensitive: false,
                    is_regex: false,
                    results,
                    results_query: Some("fn".to_string()),
                    selected: 0,
                    error: None,
                    running: false,
                    active_search: None,
                    focus: cx.focus_handle(),
                }
            })
        });
        let agent_panel = cx.new(|cx| AgentPanel::new(theme.clone(), cx));
        let active = active.min(projects.len().saturating_sub(1));
        let mut workspace = Workspace {
            projects,
            active,
            editor: None,
            split_editor: None,
            agent_panel,
            theme,
            focus_handle: cx.focus_handle(),
            state_path,
            show_left: true,
            show_right: true, // Agent パネル（差別化の本丸）は既定で表示
            show_bottom: false,
            agent_width: AGENT_DOCK_WIDTH,
            resizing_agent: false,
            resize_start_x: 0.0,
            resize_start_width: 0.0,
            explorer_width: DOCK_WIDTH,
            resizing_explorer: false,
            should_move_window: false,
            _editor_observation: None,
            picker: None,
            picker_mode: PickerMode::Files,
            picker_files: Vec::new(),
            picker_themes: Vec::new(),
            theme_before_preview: None,
            _picker_observation: None,
            // 開発用: SHIRUSHI_EXPLORER_VIEW=icons|columns で初期表示モードを指定（撮影確認用）。
            explorer_view: match std::env::var("SHIRUSHI_EXPLORER_VIEW").as_deref() {
                Ok("icons") => ExplorerView::Icons,
                Ok("columns") => ExplorerView::Columns,
                _ => ExplorerView::Tree,
            },
            // 開発用: SHIRUSHI_CONTEXT_MENU=1 でルートの右クリックメニューを開いた状態で撮る。
            explorer_context_menu,
            search_panel,
            next_search_id: 1,
            git_status: HashMap::new(),
            git_changes: Vec::new(),
            git_history: Vec::new(),
            github_slug: None,
            branch_menu: None,
            terminal: None,
            lsp: None,
            lsp_root: None,
            lsp_language: None,
            lsp_initialized: false,
            lsp_sent_version: u64::MAX,
            diagnostics: HashMap::new(),
            _lsp_pump: None,
            completion: None,
            git_panel: None,
            git_busy: false,
        };
        workspace.refresh_git_status(); // ツリー/タブの git 色分け用
        // 開発用: SHIRUSHI_GIT_PANEL=1 で git 操作パネル（ソース管理）を開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_GIT_PANEL").is_some() {
            workspace.git_panel =
                Some(GitPanelState { message: String::new(), branch_name: None, focus: cx.focus_handle() });
            workspace.refresh_git_status();
        }
        // 開発用: SHIRUSHI_BRANCH_MENU=1 で branch/worktree メニューを開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_BRANCH_MENU").is_some() {
            workspace.toggle_branch_menu(point(px(90.), px(44.)), cx);
        }
        // 開発用: SHIRUSHI_TERMINAL=1 で下ドックのターミナルを開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_TERMINAL").is_some() {
            workspace.show_bottom = true;
            workspace.ensure_terminal(cx);
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
                selected: 1,
                position: point(px(380.), px(210.)),
                focus: cx.focus_handle(),
            });
        }
        workspace.update_agent_destination(cx); // 宛先チップにプロジェクト/ブランチを反映
        workspace.save_state(); // 起動時点で状態を書く（再起動復元のため）
        workspace
    }

    /// 起動後に、アクティブプロジェクトの前回開ファイルがあれば開く。
    pub fn restore_open_file(&mut self, open_files: &[Option<PathBuf>], window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Some(path)) = open_files.get(self.active) {
            let path = path.clone();
            self.open_file(path, window, cx);
        }
    }

    fn active_slot(&self) -> Option<&ProjectSlot> {
        self.projects.get(self.active)
    }

    fn active_worktree(&self) -> Option<Rc<Worktree>> {
        self.active_slot().map(|slot| slot.worktree.clone())
    }

    /// アクティブプロジェクトの git 状態を読み直す（ツリー/タブの色分け）。git 無し/失敗は空。
    /// ディスク状態を反映するので、切替・オープン時に呼ぶ（編集中の未保存差分は gutter が担う）。
    fn refresh_git_status(&mut self) {
        let Some(worktree) = self.active_worktree() else {
            self.git_status.clear();
            self.git_changes.clear();
            self.git_history.clear();
            self.github_slug = None;
            return;
        };
        self.git_status = worktree.git_status().into_iter().collect();
        let branch = worktree.git_current_branch();
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.branch = branch;
        }
        if self.git_panel.is_some() {
            self.git_changes = worktree.git_changes();
            self.git_history = worktree.git_log_graph(30);
            // origin が GitHub かは滅多に変わらないが、パネル更新時にまとめて拾う。
            self.github_slug = project::github_slug_on(worktree.host().as_ref(), worktree.root());
        } else {
            self.git_changes.clear();
            self.git_history.clear();
            self.github_slug = None;
        }
    }

    /// git 状態の色（UI-SPEC §1.3: 色は識別に集約。theme の診断/git トークンを流用）。
    fn git_tint(theme: &Theme, status: StatusKind) -> Hsla {
        match status {
            StatusKind::Untracked | StatusKind::Added => theme.ok, // 緑
            StatusKind::Modified => theme.warn,                    // 琥珀
            StatusKind::Deleted | StatusKind::Conflicted => theme.err, // 赤
        }
    }

    /// git 状態の 1 文字バッジ（ツリー行末に出す）。
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

    /// titlebar の ⎇ クリックで branch/worktree メニューを開閉する。
    fn toggle_branch_menu(&mut self, position: Point<gpui::Pixels>, cx: &mut Context<Self>) {
        if self.branch_menu.take().is_none() {
            if let Some(worktree) = self.active_worktree() {
                self.branch_menu = Some(BranchMenuState {
                    position,
                    current: worktree.git_current_branch(),
                    branches: worktree.git_branches(),
                    worktrees: worktree.git_worktrees(),
                });
            }
        }
        cx.notify();
    }

    fn hide_branch_menu(&mut self, cx: &mut Context<Self>) {
        if self.branch_menu.take().is_some() {
            cx.notify();
        }
    }

    /// ブランチを in-place で切り替える（git switch）→ プロジェクト再読込。dirty で失敗したらログのみ。
    fn switch_branch_to(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_menu = None;
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        match worktree.switch_branch(&branch) {
            Ok(()) => self.reload_active_project(window, cx),
            Err(error) => {
                eprintln!("ブランチ切替に失敗: {error:#}");
                cx.notify();
            }
        }
    }

    /// ブランチを worktree として**新しいウィンドウ**で開く（当初ビジョン: 並行ブランチ×別窓×スレッド色）。
    /// 既存 worktree があればそれを、無ければ `<repo親>/<repo名>-<branch>` に作って開く。
    fn open_branch_worktree(&mut self, branch: String, cx: &mut Context<Self>) {
        self.branch_menu = None;
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        if let Some(existing) = worktree
            .git_worktrees()
            .into_iter()
            .find(|worktree| worktree.branch.as_deref() == Some(branch.as_str()))
        {
            self.open_folder_as_window(existing.path, cx);
            return;
        }
        let repo_name = root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string());
        let sanitized = branch.replace('/', "-");
        let Some(parent) = root.parent() else {
            eprintln!("worktree の作成先を決められない（root に親が無い）");
            cx.notify();
            return;
        };
        let target = parent.join(format!("{repo_name}-{sanitized}"));
        match worktree.add_worktree(&target, &branch) {
            Ok(()) => self.open_folder_as_window(target, cx),
            Err(error) => {
                eprintln!("worktree 作成に失敗: {error:#}");
                cx.notify();
            }
        }
    }

    /// worktree のパスを新しいウィンドウで開く。
    fn open_worktree_window(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.branch_menu = None;
        self.open_folder_as_window(path, cx);
    }

    /// ブランチ切替後などにアクティブプロジェクトを再読込（ツリー再構築・開ファイル再読込・git 更新）。
    fn reload_active_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.refresh();
        }
        let open_file = self.active_slot().and_then(|slot| slot.open_file.clone());
        let host = self.active_worktree().map(|worktree| worktree.host().clone());
        match open_file {
            Some(path) if host.as_ref().is_some_and(|host| host.metadata(&path).is_ok()) => {
                self.open_file(path, window, cx)
            }
            Some(_) => self.close_active_editor(cx),
            None => self.refresh_git_status(),
        }
        self.update_agent_destination(cx);
        cx.notify();
    }

    // ── git 操作パネル（M8: ソース管理。commit / stage / push / pull / 新規ブランチ） ──

    /// git 操作パネルをエクスプローラと切り替える（⌃⇧G）。開くと左カラムを占有しフォーカスを取る。
    fn toggle_git_panel(&mut self, _: &ToggleGitPanel, window: &mut Window, cx: &mut Context<Self>) {
        match self.git_panel.take() {
            Some(_) => {
                // 閉じる → エディタがあればフォーカスを戻す。
                if let Some(editor) = &self.editor {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            None => {
                self.show_left = true;
                let state =
                    GitPanelState { message: String::new(), branch_name: None, focus: cx.focus_handle() };
                window.focus(&state.focus, cx);
                self.git_panel = Some(state);
                self.refresh_git_status();
            }
        }
        cx.notify();
    }

    /// git パネルのキー入力（コミットメッセージ / ブランチ名を手書きで積む。検索パネルと同流儀）。
    fn on_git_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                // ブランチ名モードなら入力だけ畳む。そうでなければパネルを閉じる。
                if let Some(state) = self.git_panel.as_mut() {
                    if state.branch_name.is_some() {
                        state.branch_name = None;
                        cx.notify();
                        return;
                    }
                }
                self.toggle_git_panel(&ToggleGitPanel, window, cx);
            }
            "enter" => {
                let naming = self.git_panel.as_ref().is_some_and(|s| s.branch_name.is_some());
                if naming {
                    self.confirm_new_branch(window, cx);
                } else if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
                    self.git_commit(window, cx); // ⌘/⌃⏎ = コミット
                } else if let Some(state) = self.git_panel.as_mut() {
                    state.message.push('\n'); // 素の Enter は改行
                    cx.notify();
                }
            }
            "backspace" => {
                if let Some(state) = self.git_panel.as_mut() {
                    match &mut state.branch_name {
                        Some(name) => {
                            name.pop();
                        }
                        None => {
                            state.message.pop();
                        }
                    }
                    cx.notify();
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty() && !text.chars().any(char::is_control) {
                        if let Some(state) = self.git_panel.as_mut() {
                            match &mut state.branch_name {
                                Some(name) => name.push_str(text),
                                None => state.message.push_str(text),
                            }
                            cx.notify();
                        }
                    }
                }
            }
        }
    }

    /// staged 変更をコミット。staged が無ければ全変更を stage してからコミット（簡便動線）。
    fn git_commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let message = self.git_panel.as_ref().map(|state| state.message.clone()).unwrap_or_default();
        if message.trim().is_empty() {
            return;
        }
        let changes = worktree.git_changes();
        if changes.is_empty() {
            return;
        }
        if !changes.iter().any(|change| change.staged.is_some()) {
            if let Err(error) = worktree.stage_all() {
                eprintln!("stage に失敗: {error:#}");
                return;
            }
        }
        match worktree.commit(&message) {
            Ok(()) => {
                if let Some(state) = self.git_panel.as_mut() {
                    state.message.clear();
                }
                self.refresh_git_status();
            }
            Err(error) => eprintln!("コミットに失敗: {error:#}"),
        }
        cx.notify();
    }

    /// 1 ファイルを stage。
    fn git_stage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(worktree) = self.active_worktree() {
            if let Err(error) = worktree.stage_path(&path) {
                eprintln!("stage に失敗: {error:#}");
            }
            self.refresh_git_status();
            cx.notify();
        }
    }

    /// 1 ファイルを unstage。
    fn git_unstage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(worktree) = self.active_worktree() {
            if let Err(error) = worktree.unstage_path(&path) {
                eprintln!("unstage に失敗: {error:#}");
            }
            self.refresh_git_status();
            cx.notify();
        }
    }

    /// 全変更を stage。
    fn git_stage_all(&mut self, cx: &mut Context<Self>) {
        if let Some(worktree) = self.active_worktree() {
            if let Err(error) = worktree.stage_all() {
                eprintln!("stage に失敗: {error:#}");
            }
            self.refresh_git_status();
            cx.notify();
        }
    }

    /// push（背景実行）。UI を固めない。
    fn git_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_git_remote(true, window, cx);
    }

    /// pull（背景実行）。完了後にプロジェクトを再読込（fast-forward でファイルが変わり得る）。
    fn git_pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_git_remote(false, window, cx);
    }

    /// push/pull をバックグラウンドエグゼキュータで走らせ、完了後に git 状態を更新する。
    fn run_git_remote(&mut self, is_push: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_busy {
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
        self.git_busy = true;
        cx.notify();
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
                workspace.git_busy = false;
                match result {
                    Ok(()) if !is_push => workspace.reload_active_project(window, cx),
                    Ok(()) => workspace.refresh_git_status(),
                    Err(error) => {
                        eprintln!("{} に失敗: {error:#}", if is_push { "push" } else { "pull" });
                        workspace.refresh_git_status();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// GitHub PR 操作（`gh`・背景実行）。`create=true` で PR 作成ページ、false で PR/リポジトリを開く。
    /// git と同じ host 上で走るので remote プロジェクトでもそのまま動く（ブラウザは gh に委ねる）。
    fn github_action(&mut self, create: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_busy {
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
        self.git_busy = true;
        cx.notify();
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
                workspace.git_busy = false;
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
    fn generate_commit_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_busy {
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
        self.git_busy = true;
        cx.notify();
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::ai_commit_message_on(host.as_ref(), &root) })
                .await;
            let _ = handle.update(cx, |workspace, _window, cx| {
                workspace.git_busy = false;
                match result {
                    Ok(message) => {
                        if let Some(state) = workspace.git_panel.as_mut() {
                            state.message = message;
                        }
                    }
                    Err(error) => eprintln!("コミットメッセージ生成に失敗: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// git パネルの入力行を「新しいブランチ名」モードにする（＋ボタン）。
    fn start_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.git_panel.as_mut() {
            state.branch_name = Some(String::new());
            let focus = state.focus.clone();
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    /// 入力中のブランチ名で作成＆切替 → プロジェクト再読込。
    fn confirm_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .git_panel
            .as_ref()
            .and_then(|state| state.branch_name.clone())
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            if let Some(state) = self.git_panel.as_mut() {
                state.branch_name = None;
            }
            cx.notify();
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        match worktree.create_branch(&name) {
            Ok(()) => {
                if let Some(state) = self.git_panel.as_mut() {
                    state.branch_name = None;
                }
                self.reload_active_project(window, cx);
            }
            Err(error) => {
                eprintln!("ブランチ作成に失敗: {error:#}");
                cx.notify();
            }
        }
    }

    /// ⎇ メニューからブランチを削除（未マージは git が `-d` で拒否＝安全側）。
    fn delete_git_branch(&mut self, branch: String, cx: &mut Context<Self>) {
        self.branch_menu = None;
        if let Some(worktree) = self.active_worktree() {
            if let Err(error) = worktree.delete_branch(&branch, false) {
                eprintln!("ブランチ削除に失敗（未マージは別窓/worktree で対応）: {error:#}");
            }
            self.refresh_git_status();
            cx.notify();
        }
    }

    // ── LSP（言語サーバ・M7。拡張子→サーバの登録式） ──

    /// アクティブファイルの言語に合った言語サーバを（必要なら）起動する。
    /// 別プロジェクト or 別言語に移ったら張り替える。サーバ未登録の拡張子では何もしない
    /// （既存接続は温存＝別タブに戻れば診断が残る）。
    fn ensure_lsp(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let Some(path) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.read(cx).buffer().path().map(Path::to_path_buf))
        else {
            return;
        };
        let Some(server) = language_server_for(&path, worktree.host().is_remote()) else {
            return; // この拡張子に LSP は無い（＝静かに素通り）
        };
        // 同じ root × 同じ言語なら張り替え不要。
        if self.lsp.is_some()
            && self.lsp_root.as_deref() == Some(root.as_path())
            && self.lsp_language == Some(server.language_id)
        {
            return;
        }
        // 旧接続を畳む（Drop で kill）。
        self.lsp = None;
        self._lsp_pump = None;
        self.lsp_initialized = false;
        self.lsp_sent_version = u64::MAX;
        self.diagnostics.clear();
        let (client, notifications) = match lang::lsp::LspClient::new_on(
            worktree.host().clone(),
            &server.command,
            &server.args,
            &root,
        ) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("LSP の起動に失敗（{}）: {error:#}", server.language_id);
                return;
            }
        };
        let init_rx = client.initialize_request(&root);
        self.lsp = Some(client);
        self.lsp_root = Some(root);
        self.lsp_language = Some(server.language_id);
        // pump: initialize 応答 → initialized + 現在ファイル didOpen → 以後 publishDiagnostics を処理。
        self._lsp_pump = Some(cx.spawn(async move |workspace, cx| {
            let _ = init_rx.await; // capabilities は今は使わない（起動確認のみ）
            if workspace.update(cx, |ws, cx| ws.on_lsp_initialized(cx)).is_err() {
                return;
            }
            let mut notifications = notifications;
            while let Some((method, params)) = notifications.next().await {
                if method == "textDocument/publishDiagnostics"
                    && workspace.update(cx, |ws, cx| ws.on_diagnostics(params, cx)).is_err()
                {
                    break;
                }
            }
        }));
    }

    /// initialize 応答後: initialized 通知 + 現在の rust ファイルを didOpen。
    fn on_lsp_initialized(&mut self, cx: &mut Context<Self>) {
        self.lsp_initialized = true;
        if let Some(lsp) = &self.lsp {
            lsp.initialized();
        }
        self.lsp_did_open_active(cx);
    }

    /// 現在開いているファイルを didOpen する（稼働中サーバが担当する言語に限る）。
    fn lsp_did_open_active(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let info = {
            let view = editor.read(cx);
            match view.buffer().path() {
                // 稼働中サーバが担当する言語のときだけ開く。
                Some(path) => language_server_for(path, view.buffer().host().is_remote())
                    .filter(|server| self.lsp_language == Some(server.language_id))
                    .map(|server| {
                        (path.to_path_buf(), server.language_id, view.buffer().version(), view.buffer().text())
                    }),
                None => None,
            }
        };
        if let Some((path, language_id, version, text)) = info {
            self.lsp_sent_version = version;
            if let Some(lsp) = &self.lsp {
                lsp.did_open(&path, language_id, version as i32, &text);
            }
        }
    }

    /// publishDiagnostics を受けてファイル別に格納し、アクティブファイル分をエディタへ push。
    fn on_diagnostics(&mut self, params: serde_json::Value, cx: &mut Context<Self>) {
        let Ok(parsed) = serde_json::from_value::<lang::lsp::PublishDiagnosticsParams>(params) else {
            return;
        };
        let Some(path) = lang::lsp::uri_to_path(&parsed.uri) else {
            return;
        };
        let entries: Vec<(u32, lang::lsp::Severity)> = parsed
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (diagnostic.range.start.line, lang::lsp::Severity::from_lsp(diagnostic.severity))
            })
            .collect();
        // 空 vec = そのファイルの診断を全消し（置換セマンティクス）。
        if entries.is_empty() {
            self.diagnostics.remove(&path);
        } else {
            self.diagnostics.insert(path, entries);
        }
        self.push_active_diagnostics(cx);
        cx.notify();
    }

    /// アクティブファイルの診断をエディタへ渡す（gutter 下線用）。
    fn push_active_diagnostics(&self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let path = editor.read(cx).buffer().path().map(Path::to_path_buf);
        let diagnostics = path
            .and_then(|path| self.diagnostics.get(&path).cloned())
            .unwrap_or_default();
        editor.update(cx, |view, cx| view.set_diagnostics(diagnostics, cx));
    }

    /// エディタ変更時（observe）: 再描画 + LSP didChange（rust・version 変化時のみ）。
    fn on_editor_changed(&mut self, editor: Entity<EditorView>, cx: &mut Context<Self>) {
        cx.notify();
        // 初期化 + didOpen 前は didChange を送らない（さもないと ra が「initialized 前」で落ちる）。
        // observe は focus/blink 等の notify でも発火するので version 変化でのみ送る。
        if !self.lsp_initialized {
            return;
        }
        let info = {
            let view = editor.read(cx);
            let version = view.buffer().version();
            if version == self.lsp_sent_version {
                None
            } else {
                view.buffer()
                    .path()
                    .filter(|path| {
                        language_server_for(path, view.buffer().host().is_remote()).is_some()
                    })
                    .map(|path| (path.to_path_buf(), version, view.buffer().text()))
            }
        };
        if let Some((path, version, text)) = info {
            self.lsp_sent_version = version;
            if let Some(lsp) = &self.lsp {
                lsp.did_change(&path, version as i32, &text);
            }
        }
    }

    /// アクティブファイルの診断件数（error, warning）。statusbar 用。
    fn active_diagnostic_counts(&self, cx: &App) -> (usize, usize) {
        let Some(editor) = &self.editor else {
            return (0, 0);
        };
        let Some(path) = editor.read(cx).buffer().path().map(Path::to_path_buf) else {
            return (0, 0);
        };
        let Some(entries) = self.diagnostics.get(&path) else {
            return (0, 0);
        };
        let errors = entries.iter().filter(|(_, severity)| *severity == lang::lsp::Severity::Error).count();
        let warnings =
            entries.iter().filter(|(_, severity)| *severity == lang::lsp::Severity::Warning).count();
        (errors, warnings)
    }

    /// 定義ジャンプ（F12）。カーソル位置の定義を rust-analyzer に問い合わせて着地する。
    fn go_to_definition(&mut self, _: &GoToDefinition, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let info = {
            let view = editor.read(cx);
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(|path| {
                let (line, character) = view.cursor_lsp_position();
                (path.to_path_buf(), line, character)
            })
        };
        let Some((path, line, character)) = info else {
            return;
        };
        let Some(lsp) = &self.lsp else {
            return;
        };
        let receiver = lsp.definition(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            if let Some((target, target_line, target_character)) = parse_definition(&value) {
                let _ = handle.update(cx, |workspace, window, cx| {
                    workspace.jump_to_location(target, target_line, target_character, window, cx)
                });
            }
        })
        .detach();
    }

    /// 定義の着地: 別ファイルなら開き、対象位置を中央へ寄せる。
    fn jump_to_location(
        &mut self,
        path: PathBuf,
        line: u32,
        character: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .editor
            .as_ref()
            .and_then(|editor| editor.read(cx).buffer().path().map(Path::to_path_buf));
        if current.as_deref() != Some(path.as_path()) {
            self.open_file(path, window, cx);
        }
        if let Some(editor) = &self.editor {
            editor.update(cx, |view, cx| view.reveal_lsp_position(line, character, cx));
        }
        cx.notify();
    }

    /// 補完（Ctrl-Space）。カーソル位置で候補を取得しポップアップを出す。
    fn trigger_completion(&mut self, _: &TriggerCompletion, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let info = {
            let view = editor.read(cx);
            let position = view.caret_window_position();
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(|path| {
                let (line, character) = view.cursor_lsp_position();
                (path.to_path_buf(), line, character, position)
            })
        };
        let Some((path, line, character, position)) = info else {
            return;
        };
        let Some(lsp) = &self.lsp else {
            return;
        };
        let receiver = lsp.completion(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                workspace.show_completion(&value, position, window, cx)
            });
        })
        .detach();
    }

    fn show_completion(
        &mut self,
        value: &serde_json::Value,
        position: Option<Point<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = parse_completion_items(value);
        if items.is_empty() {
            return;
        }
        let focus = cx.focus_handle();
        let position = position.unwrap_or_else(|| point(px(220.), px(180.)));
        window.focus(&focus, cx);
        self.completion = Some(CompletionState { items, selected: 0, position, focus });
        cx.notify();
    }

    fn close_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.completion.take().is_none() {
            return;
        }
        if let Some(editor) = &self.editor {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn move_completion_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(state) = self.completion.as_mut() {
            let len = state.items.len() as isize;
            if len == 0 {
                return;
            }
            state.selected = (state.selected as isize + delta).rem_euclid(len) as usize;
            cx.notify();
        }
    }

    fn confirm_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let insert = self
            .completion
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .map(|item| item.insert_text.clone());
        self.completion = None;
        if let Some(editor) = self.editor.clone() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            if let Some(text) = insert {
                editor.update(cx, |view, cx| view.apply_completion(&text, cx));
            }
        }
        cx.notify();
    }

    fn on_completion_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_completion(window, cx),
            "up" => self.move_completion_selection(-1, cx),
            "down" => self.move_completion_selection(1, cx),
            "enter" | "tab" => self.confirm_completion(window, cx),
            // ナビゲーション以外はポップアップを閉じてエディタへ戻す（v1: 絞り込みは再トリガ）。
            _ => self.close_completion(window, cx),
        }
    }

    fn switch_project(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.projects.len() || index == self.active {
            return;
        }
        self.active = index;
        // process/PTY は project の host と不可分。旧 host の session を次の project へ持ち越さない。
        self.terminal = None;
        self.lsp = None;
        self._lsp_pump = None;
        self.lsp_root = None;
        self.lsp_language = None;
        self.lsp_initialized = false;
        self.lsp_sent_version = u64::MAX;
        self.diagnostics.clear();
        // 切替先プロジェクトの開ファイルを復元（無ければエディタを閉じる）
        let open_file = self.projects[index].open_file.clone();
        match open_file {
            Some(path) => self.open_file(path, window, cx),
            None => {
                self.editor = None;
                self._editor_observation = None;
            }
        }
        self.refresh_git_status();
        self.update_agent_destination(cx);
        if self.show_bottom {
            self.ensure_terminal(cx);
        }
        self.save_state();
        cx.notify();
    }

    /// Agent パネルの宛先チップにアクティブプロジェクト名・ブランチを反映する。
    fn update_agent_destination(&self, cx: &mut Context<Self>) {
        let (name, branch, host, cwd, files) = match self.active_slot() {
            Some(slot) => {
                // Add context の候補（プロジェクト先頭 60 ファイルの相対パス）。
                let files = slot
                    .worktree
                    .all_files(60)
                    .into_iter()
                    .map(|(_, relative)| SharedString::from(relative))
                    .collect();
                (
                    slot.name.clone(),
                    slot.branch.clone().map(SharedString::from),
                    slot.worktree.host().clone(),
                    Some(slot.worktree.root().to_path_buf()),
                    files,
                )
            }
            None => (
                SharedString::from("—"),
                None,
                host::LocalHost::shared(),
                None,
                Vec::new(),
            ),
        };
        self.agent_panel
            .update(cx, |panel, cx| panel.set_destination(name, branch, host, cwd, files, cx));
    }

    // ── タブ/スレッドのショートカット（⌘W / ⌘⇧A） ──

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        // Agent パネルにフォーカスがあるなら AI スレッドタブを閉じる（Chrome 風 ⌘W）。
        // gpui は no-context バインドを最深で解決する（keymap では分離不能）ので、ここで振り分ける。
        let agent_focused =
            self.show_right && self.agent_panel.read(cx).focus_handle(cx).contains_focused(window, cx);
        if agent_focused {
            self.agent_panel.update(cx, |panel, cx| panel.close_active_thread(cx));
            return;
        }
        self.close_active_editor(cx);
    }

    fn new_agent_thread(&mut self, _: &NewThread, _: &mut Window, cx: &mut Context<Self>) {
        if !self.show_right {
            self.show_right = true;
        }
        self.agent_panel.update(cx, |panel, cx| panel.new_thread(cx));
        cx.notify();
    }

    /// 次の AI スレッドタブへ（Chrome 風。⌘⌥→ / ⌃Tab）。
    fn select_next_thread(&mut self, _: &SelectNextThread, _: &mut Window, cx: &mut Context<Self>) {
        if !self.show_right {
            self.show_right = true;
        }
        self.agent_panel.update(cx, |panel, cx| panel.select_next_thread(cx));
        cx.notify();
    }

    /// 前の AI スレッドタブへ（Chrome 風。⌘⌥← / ⌃⇧Tab）。
    fn select_prev_thread(&mut self, _: &SelectPrevThread, _: &mut Window, cx: &mut Context<Self>) {
        if !self.show_right {
            self.show_right = true;
        }
        self.agent_panel.update(cx, |panel, cx| panel.select_prev_thread(cx));
        cx.notify();
    }

    /// アクティブプロジェクトを**新しいウィンドウ**で開く（⌘⇧N。ウィンドウモデル §5）。
    fn new_window(&mut self, _: &NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(slot) = self.active_slot() {
            let root = slot.worktree.root().to_path_buf();
            self.open_folder_as_window(root, cx);
        }
    }

    // ── ドックの可変幅（縁ドラッグ）。Agent=左縁 / エクスプローラ=右縁 ──

    fn on_resize_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dx = f32::from(event.position.x) - self.resize_start_x;
        if self.resizing_agent {
            // 左縁を左へ動かすと広がる（dx 負 → 幅増）。
            self.agent_width = (self.resize_start_width - dx).clamp(AGENT_DOCK_MIN, AGENT_DOCK_MAX);
            cx.notify();
        } else if self.resizing_explorer {
            // 右縁を右へ動かすと広がる（dx 正 → 幅増）。
            self.explorer_width = (self.resize_start_width + dx).clamp(DOCK_MIN, DOCK_MAX);
            cx.notify();
        }
    }

    fn on_resize_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.resizing_agent || self.resizing_explorer {
            self.resizing_agent = false;
            self.resizing_explorer = false;
            cx.notify();
        }
    }

    /// Agent パネルを可変幅コンテナに入れて描く（左縁にリサイズハンドル）。
    fn render_agent_dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .flex_none()
            .w(px(self.agent_width))
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("agent-resize")
                    .w(px(RESIZE_HANDLE_WIDTH))
                    .h_full()
                    .flex_none()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|style| style.bg(theme.border))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.resizing_agent = true;
                            this.resize_start_x = f32::from(event.position.x);
                            this.resize_start_width = this.agent_width;
                            cx.notify();
                        }),
                    ),
            )
            .child(div().flex_1().min_w_0().h_full().child(self.agent_panel.clone()))
    }

    fn toggle_dock(&mut self, dock: Dock, cx: &mut Context<Self>) {
        match dock {
            Dock::Left => self.show_left = !self.show_left,
            Dock::Right => self.show_right = !self.show_right,
            Dock::Bottom => self.show_bottom = !self.show_bottom,
        }
        cx.notify();
    }

    // ── 下ドックのターミナル（M8） ──

    /// ターミナルを遅延生成する（初回表示時。cwd = アクティブプロジェクトルート）。
    fn ensure_terminal(&mut self, cx: &mut Context<Self>) -> Entity<TerminalView> {
        if let Some(terminal) = &self.terminal {
            return terminal.clone();
        }
        let (cwd, shell) = self
            .active_slot()
            .map(|slot| {
                let root = slot.worktree.root().to_path_buf();
                match slot.worktree.host().terminal_launch(&root) {
                    Ok(Some(launch)) => (None, Some((launch.program, launch.args))),
                    Ok(None) => (Some(root), None),
                    Err(error) => {
                        eprintln!("remote terminal を起動できない: {error:#}");
                        (None, None)
                    }
                }
            })
            .unwrap_or((None, None));
        let theme = self.theme.clone();
        let terminal = cx.new(|cx| TerminalView::new_with_shell(cwd, shell, theme, cx));
        self.terminal = Some(terminal.clone());
        terminal
    }

    /// 下ドック（ターミナル）を開閉する。開くときは生成 + フォーカス（キー入力を受ける）。
    fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.show_bottom = !self.show_bottom;
        if self.show_bottom {
            let terminal = self.ensure_terminal(cx);
            let handle = terminal.read(cx).focus_handle();
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// アクティブタブ（＝現在のエディタ）を閉じる。
    fn close_active_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.split_editor = None; // 主ペインを閉じたら分割も畳む
        self._editor_observation = None;
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.open_file = None;
            slot.selected = None;
        }
        self.save_state();
        cx.notify();
    }

    /// 右分割ペインを開閉する（⌘\）。開くときは主ペインの開いているファイルを独立エディタで複製する
    /// （比較・参照用の副ビュー。LSP/保存の統合は主ペイン=editor 側が担う）。
    fn toggle_split(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_editor.is_some() {
            self.close_split(window, cx);
            return;
        }
        let Some(path) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.read(cx).buffer().path().map(Path::to_path_buf))
        else {
            return;
        };
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let buffer = match Buffer::from_host(worktree.host().clone(), &path) {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("分割ペインを開けない: {error:#}");
                return;
            }
        };
        let theme = self.theme.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let split = cx.new(|cx| EditorView::new(buffer, theme, accent, cx));
        let handle = split.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.split_editor = Some(split);
        cx.notify();
    }

    fn close_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_editor.take().is_none() {
            return;
        }
        if let Some(editor) = &self.editor {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn toggle_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(slot) = self.projects.get_mut(self.active) {
            if slot.expanded.contains(&path) {
                slot.expanded.remove(&path);
            } else {
                slot.expanded.insert(path);
            }
            slot.refresh();
            cx.notify();
        }
    }

    /// エクスプローラの表示モードを切り替える（左下スイッチャー）。
    /// カラム表示は幅が要るので、狭ければ広げる（以後ユーザーがドラッグで調整）。
    fn set_explorer_view(&mut self, view: ExplorerView, cx: &mut Context<Self>) {
        self.explorer_view = view;
        if view == ExplorerView::Columns && self.explorer_width < 440.0 {
            self.explorer_width = 440.0;
        }
        cx.notify();
    }

    /// カラム/アイコン表示で `dir` に入る（現在フォルダを更新）。ブレッドクラムの上位階層クリックでも使う。
    /// **ルート外へ出た場合**（隣のリポジトリへ辿る）は、ツリー表示だと current_dir を反映できないので
    /// カラム表示（Finder 風）へ自動で切り替える（M5 受入: マウスだけで上へ辿る）。
    fn enter_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let outside = self
            .active_slot()
            .map(|slot| !dir.starts_with(slot.worktree.root()))
            .unwrap_or(false);
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.current_dir = Some(dir.clone());
            slot.selected = Some(dir);
        }
        if outside && self.explorer_view == ExplorerView::Tree {
            self.explorer_view = ExplorerView::Columns;
        }
        cx.notify();
    }

    /// エクスプローラの右クリックメニューを出す（対象と位置を記録）。
    fn show_context_menu(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.explorer_context_menu = Some(ExplorerContextMenu { path, is_dir, position });
        cx.notify();
    }

    /// 右クリックメニューを閉じる（外側クリック・アクション実行後）。
    fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.explorer_context_menu.take().is_some() {
            cx.notify();
        }
    }

    /// フォルダを**新規ウィンドウ**でプロジェクトとして開く（ウィンドウモデルの核）。
    fn open_folder_as_window(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let source = match self.active_worktree() {
            Some(worktree) => match worktree.host().host_for_project(&path) {
                Ok(host) => ProjectSource::new(host, path),
                Err(error) => {
                    eprintln!("別 project を開けない: {error:#}");
                    return;
                }
            },
            None => ProjectSource::local(path),
        };
        let theme = self.theme.clone();
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Shirushi".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(13.0), px(13.0))),
                }),
                is_movable: false,
                ..Default::default()
            },
            move |_window, cx| {
                cx.new(|cx| {
                    Workspace::new_sources(
                        vec![source.clone()],
                        theme.clone(),
                        state_path(),
                        cx,
                    )
                })
            },
        );
        if let Err(error) = opened {
            eprintln!("新規ウィンドウを開けない: {error}");
        }
        self.explorer_context_menu = None;
        cx.notify();
    }

    /// パスをクリップボードへコピー。
    fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        self.explorer_context_menu = None;
        cx.notify();
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let buffer = match Buffer::from_host(worktree.host().clone(), &path) {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("ファイルを開けない: {error:#}");
                return;
            }
        };
        let theme = self.theme.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let editor = cx.new(|cx| EditorView::new(buffer, theme, accent, cx));

        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        // 変更を監視（再描画 + LSP didChange）。
        self._editor_observation = Some(cx.observe(&editor, Self::on_editor_changed));
        self.editor = Some(editor);

        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.selected = Some(path.clone());
            slot.open_file = Some(path);
        }
        self.refresh_git_status();
        // LSP: この拡張子にサーバがあれば起動 + didOpen（初期化済みなら即 didOpen）。
        let has_language_server = self
            .editor
            .as_ref()
            .and_then(|editor| {
                let view = editor.read(cx);
                view.buffer().path().map(|path| {
                    language_server_for(path, view.buffer().host().is_remote()).is_some()
                })
            })
            .unwrap_or(false);
        if has_language_server {
            self.ensure_lsp(cx);
            if self.lsp_initialized {
                self.lsp_did_open_active(cx);
            }
            // 既知の診断があれば即反映。
            self.push_active_diagnostics(cx);
        }
        // 開発用: SHIRUSHI_SPLIT=1 で右分割ペインを開いた状態で撮る。
        if self.split_editor.is_none() && std::env::var_os("SHIRUSHI_SPLIT").is_some() {
            self.toggle_split(&SplitRight, window, cx);
        }
        self.save_state();
        cx.notify();
    }

    fn save_state(&self) {
        let Some(path) = self.state_path.as_ref() else {
            return;
        };
        let state = PersistedState {
            projects: self
                .projects
                .iter()
                .map(|slot| PersistedProject {
                    root: slot.worktree.root().to_path_buf(),
                    open_file: slot.open_file.clone(),
                    remote_uri: slot.worktree.host().project_uri(slot.worktree.root()),
                })
                .collect(),
            active: self.active,
        };
        let Ok(text) = serde_json::to_string_pretty(&state) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(path, text) {
            eprintln!("状態の保存に失敗: {error}");
        }
    }

    /// 復元用: 各プロジェクトの前回開ファイル。
    pub fn persisted_open_files(state_path: &Path) -> Vec<Option<PathBuf>> {
        let Ok(text) = std::fs::read_to_string(state_path) else {
            return Vec::new();
        };
        let Ok(state) = serde_json::from_str::<PersistedState>(&text) else {
            return Vec::new();
        };
        state.projects.into_iter().map(|project| project.open_file).collect()
    }

    // ── オーバーレイ（Picker） ──

    fn open_file_finder(&mut self, _: &FileFinder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(slot) = self.active_slot() else {
            return;
        };
        let files = slot.worktree.all_files(5000);
        let items = files
            .iter()
            .enumerate()
            .map(|(id, (_, relative))| PickerItem::new(id, relative.clone()))
            .collect();
        self.picker_files = files.into_iter().map(|(path, _)| path).collect();
        self.open_picker(PickerMode::Files, i18n::t!("finder.files"), items, window, cx);
    }

    fn open_project_switcher(&mut self, _: &ProjectSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        let items = self
            .projects
            .iter()
            .enumerate()
            .map(|(id, slot)| {
                PickerItem::new(id, slot.name.clone())
                    .with_detail(slot.worktree.root().display().to_string())
            })
            .collect();
        self.open_picker(PickerMode::Projects, i18n::t!("finder.projects"), items, window, cx);
    }

    // ── テーマセレクタ（Picker・ライブプレビュー付き。⌘⇧T・M3） ──

    /// テーマセレクタを開く。組み込み + ユーザーテーマを Picker に並べ、選択移動で即プレビューする。
    fn open_theme_selector(&mut self, _: &ThemeSelector, window: &mut Window, cx: &mut Context<Self>) {
        let themes = theme_core::available_themes(self.themes_dir().as_deref());
        let items = themes
            .iter()
            .enumerate()
            .map(|(id, (name, source))| {
                let detail = match source {
                    ThemeSource::BuiltIn(_) => "組み込み",
                    ThemeSource::User(_) => "ユーザー",
                };
                PickerItem::new(id, name.clone()).with_detail(detail)
            })
            .collect();
        self.picker_themes = themes;
        self.theme_before_preview = Some(self.theme.clone());
        self.open_picker(PickerMode::Themes, "テーマを選択".to_string(), items, window, cx);
    }

    /// テーマ保存ディレクトリ（`state.json` と同じ Shirushi 設定フォルダの `themes/`）。
    fn themes_dir(&self) -> Option<PathBuf> {
        self.state_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|dir| dir.join("themes"))
    }

    /// テーマを即時適用する（自身のクローム + エディタ + Agent パネル + Picker へ波及）。
    fn apply_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.set_theme(theme.clone(), cx));
        }
        self.agent_panel.update(cx, |panel, cx| panel.set_theme(theme.clone(), cx));
        if let Some(picker) = &self.picker {
            picker.update(cx, |picker, cx| picker.set_theme(theme.clone(), cx));
        }
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, cx| terminal.set_theme(theme.clone(), cx));
        }
        cx.notify();
    }

    /// ハイライト移動でプレビュー適用する（保存しない）。
    fn preview_theme(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some((_, source)) = self.picker_themes.get(id) {
            if let Ok(theme) = Theme::load(source) {
                self.apply_theme(theme, cx);
            }
        }
    }

    /// テーマを確定する（適用 + 設定へ theme 名を保存＝再起動でも効く）。
    fn commit_theme(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some((_, source)) = self.picker_themes.get(id).cloned() else {
            return;
        };
        let Ok(theme) = Theme::load(&source) else {
            return;
        };
        let name = theme.name.to_string();
        self.apply_theme(theme, cx);
        self.theme_before_preview = None;
        if let Some(path) = settings_core::user_settings_path() {
            if let Err(error) =
                settings_core::persist_user_value(&path, "theme", serde_json::Value::String(name))
            {
                eprintln!("テーマの保存に失敗: {error:#}");
            }
        }
    }

    fn open_picker(
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
        self._picker_observation = Some(cx.subscribe_in(&picker, window, Self::on_picker_event));
        self.picker_mode = mode;
        self.picker = Some(picker);
        cx.notify();
    }

    fn on_picker_event(
        &mut self,
        _picker: &Entity<Picker>,
        event: &PickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // テーマセレクタのみ、ハイライト移動で即プレビュー（ライブプレビュー）。
            PickerEvent::Highlighted(id) => {
                if self.picker_mode == PickerMode::Themes {
                    self.preview_theme(*id, cx);
                }
            }
            PickerEvent::Confirmed(id) => {
                let id = *id;
                let mode = self.picker_mode;
                self.close_picker(window, cx);
                match mode {
                    PickerMode::Files => {
                        if let Some(path) = self.picker_files.get(id).cloned() {
                            self.open_file(path, window, cx);
                        }
                    }
                    PickerMode::Projects => self.switch_project(id, window, cx),
                    PickerMode::Themes => self.commit_theme(id, cx),
                }
            }
            PickerEvent::Dismissed => {
                // テーマセレクタを中止したらプレビューを元へ戻す。
                if self.picker_mode == PickerMode::Themes {
                    if let Some(theme) = self.theme_before_preview.take() {
                        self.apply_theme(theme, cx);
                    }
                }
                self.close_picker(window, cx);
            }
        }
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self._picker_observation = None;
        match &self.editor {
            Some(editor) => {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            None => window.focus(&self.focus_handle, cx),
        }
        cx.notify();
    }

    // ── プロジェクト横断検索パネル（⌘⇧F・M6） ──

    /// 検索パネルを開く（開いていればフォーカスを付け直すだけ＝クエリと結果は保つ）。
    fn open_project_search(&mut self, _: &ProjectSearch, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        match &mut self.search_panel {
            Some(state) => state.focus = focus.clone(),
            None => {
                self.search_panel = Some(SearchState {
                    query: String::new(),
                    case_sensitive: false,
                    is_regex: false,
                    results: Vec::new(),
                    results_query: None,
                    selected: 0,
                    error: None,
                    running: false,
                    active_search: None,
                    focus: focus.clone(),
                })
            }
        }
        window.focus(&focus, cx);
        cx.notify();
    }

    /// 検索パネルを閉じてフォーカスをエディタ/ワークスペースへ戻す。
    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_panel.take().is_none() {
            return;
        }
        match &self.editor {
            Some(editor) => {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            None => window.focus(&self.focus_handle, cx),
        }
        cx.notify();
    }

    /// 現在のクエリ + トグルでアクティブプロジェクトを横断検索し、結果を格納する。
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let Some((query_text, is_regex, case_sensitive)) = self
            .search_panel
            .as_ref()
            .map(|state| (state.query.clone(), state.is_regex, state.case_sensitive))
        else {
            return;
        };
        // 空クエリ → 結果クリア（走査しない）。
        if query_text.trim().is_empty() {
            if let Some(state) = self.search_panel.as_mut() {
                state.results.clear();
                state.results_query = Some(query_text);
                state.error = None;
                state.selected = 0;
                state.running = false;
                state.active_search = None;
            }
            cx.notify();
            return;
        }
        let query = match search::SearchQuery::new(&query_text, is_regex, case_sensitive) {
            Ok(query) => query,
            Err(error) => {
                if let Some(state) = self.search_panel.as_mut() {
                    state.results.clear();
                    state.results_query = Some(query_text);
                    state.error = Some(SharedString::from(format!("{error}")));
                    state.selected = 0;
                    state.running = false;
                    state.active_search = None;
                }
                cx.notify();
                return;
            }
        };
        // 対象 host/root だけを background task へ渡す。UI thread では remote RPC を待たない。
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let search_id = self.next_search_id;
        self.next_search_id = self.next_search_id.wrapping_add(1).max(1);
        if let Some(state) = self.search_panel.as_mut() {
            state.running = true;
            state.active_search = Some(search_id);
            state.error = None;
        }
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    query.try_search_project_on(
                        host.as_ref(),
                        &root,
                        SEARCH_FILE_LIMIT,
                        SEARCH_MAX_ROWS,
                    )
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                let Some(state) = workspace.search_panel.as_mut() else {
                    return;
                };
                if state.active_search != Some(search_id)
                    || state.query != query_text
                    || state.is_regex != is_regex
                    || state.case_sensitive != case_sensitive
                {
                    return;
                }
                match outcome {
                    Ok(results) => {
                        state.results = results;
                        state.error = None;
                    }
                    Err(error) => {
                        state.results.clear();
                        state.error = Some(SharedString::from(format!("{error:#}")));
                    }
                }
                state.results_query = Some(query_text);
                state.selected = 0;
                state.running = false;
                state.active_search = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// 選択中マッチをキーボードで動かす（結果全体を平坦に巡回）。
    fn move_search_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(state) = self.search_panel.as_mut() {
            let len = state.flat().len() as isize;
            if len == 0 {
                return;
            }
            state.selected = (state.selected as isize + delta).rem_euclid(len) as usize;
            cx.notify();
        }
    }

    /// 検索結果の 1 マッチへジャンプ（ファイルを開いて該当行を中央へ）。
    fn jump_to_search_match(
        &mut self,
        file_index: usize,
        match_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.search_panel.as_ref().and_then(|state| {
            state.results.get(file_index).and_then(|file| {
                file.matches
                    .get(match_index)
                    .map(|found| (file.path.clone(), found.line, found.column))
            })
        });
        let Some((path, line, column)) = target else {
            return;
        };
        self.search_panel = None;
        self.open_file(path, window, cx);
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.reveal_position(line, column, cx));
        }
        cx.notify();
    }

    fn on_search_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_search(window, cx),
            "enter" => {
                // 結果が最新クエリのものなら選択項目へジャンプ、そうでなければ検索を実行する。
                let jump = self.search_panel.as_ref().and_then(|state| {
                    let is_fresh = state.results_query.as_deref() == Some(state.query.as_str());
                    let flat = state.flat();
                    if is_fresh && !flat.is_empty() {
                        flat.get(state.selected).copied()
                    } else {
                        None
                    }
                });
                match jump {
                    Some((file_index, match_index)) => {
                        self.jump_to_search_match(file_index, match_index, window, cx)
                    }
                    None => self.run_search(cx),
                }
            }
            "up" => self.move_search_selection(-1, cx),
            "down" => self.move_search_selection(1, cx),
            "backspace" => {
                if let Some(state) = self.search_panel.as_mut() {
                    state.query.pop();
                }
                cx.notify();
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty() && !text.chars().any(char::is_control) {
                        if let Some(state) = self.search_panel.as_mut() {
                            state.query.push_str(text);
                        }
                        cx.notify();
                    }
                }
            }
        }
    }

    /// 大小区別トグル（切替後は即再検索）。
    fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.search_panel.as_mut() {
            state.case_sensitive = !state.case_sensitive;
        }
        self.run_search(cx);
    }

    /// 正規表現トグル（切替後は即再検索）。
    fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.search_panel.as_mut() {
            state.is_regex = !state.is_regex;
        }
        self.run_search(cx);
    }

    // ── 描画 ──

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.active;
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .pt_2()
            .children(self.projects.iter().enumerate().map(|(index, slot)| {
                let color = slot.color;
                let is_active = index == active;
                let monogram = slot.name.chars().next().unwrap_or('•').to_string();
                let name = slot.name.clone();
                div()
                    .id(("rail-project", index))
                    .size(px(30.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.fg0)
                    .bg(color.alpha(0.14))
                    .border_2()
                    .border_color(if is_active { color } else { color.alpha(0.35) })
                    .cursor_pointer()
                    // 非アクティブは hover で色が濃くなる＝クリックできる合図（Zed の気持ちよさ）
                    .hover(|style| style.bg(color.alpha(0.24)).border_color(color))
                    .child(monogram)
                    .tooltip(Tooltip::text(name, theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.switch_project(index, window, cx)),
                    )
            }))
            // ＋ = プロジェクト/ブランチスイッチャー（⌘O）
            .child(
                div()
                    .id("rail-add")
                    .size(px(30.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.fg2)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .child("＋")
                    .hover(|style| style.text_color(theme.fg0).border_color(theme.fg2).bg(theme.bg2))
                    .tooltip(Tooltip::text("プロジェクトを開く  ⌘O", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_project_switcher(&ProjectSwitcher, window, cx)
                        }),
                    ),
            )
            .child(div().flex_1())
            .child(div().pb_1().text_size(px(9.)).text_color(theme.fg2).child("⌘O"))
    }

    fn render_explorer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let Some(slot) = self.active_slot() else {
            return div()
                .w(px(DOCK_WIDTH))
                .h_full()
                .flex_none()
                .bg(theme.bg0)
                .border_r_1()
                .border_color(theme.border);
        };
        // 表示モードで本体を切り替える。カラム表示は複数カラム分だけ横幅を広げる（Finder 風）。
        let body = match self.explorer_view {
            ExplorerView::Tree => self.render_tree(slot, cx),
            ExplorerView::Columns => self.render_columns(slot, cx),
            ExplorerView::Icons => self.render_icons(slot, cx),
        };
        div()
            .w(px(self.explorer_width))
            .h_full()
            .flex_none()
            .relative() // リサイズハンドルの絶対配置基準
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .child(self.render_explorer_header(slot, cx))
            .child(body)
            .child(self.render_explorer_footer(cx))
            // 右縁のリサイズハンドル（ドラッグで幅変更）。
            .child(
                div()
                    .id("explorer-resize")
                    .absolute()
                    .top_0()
                    .right(px(0.))
                    .w(px(RESIZE_HANDLE_WIDTH))
                    .h_full()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|style| style.bg(theme.border))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.resizing_explorer = true;
                            this.resize_start_x = f32::from(event.position.x);
                            this.resize_start_width = this.explorer_width;
                            cx.notify();
                        }),
                    ),
            )
    }

    /// ツリー表示（縦。従来）。行 = chevron + アイコン + 名前。
    fn render_tree(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let color = slot.color;
        let selected = slot.selected.clone();
        let git_status = &self.git_status;
        let root = slot.worktree.root().to_path_buf(); // ドラッグ時の @メンション相対パス用
        div()
            .flex_1()
            .overflow_hidden()
            .children(slot.rows.iter().enumerate().map(|(index, row)| {
                let path = row.path.clone();
                let is_dir = row.is_dir;
                let is_selected = selected.as_ref() == Some(&row.path);
                // git 色分け: ファイルは自身の状態、フォルダは配下に変更があれば ● を出す。
                let file_status = if is_dir { None } else { git_status.get(&row.path).copied() };
                let dir_dirty =
                    is_dir && git_status.keys().any(|changed| changed.starts_with(&row.path));
                let name_color = match file_status {
                    Some(status) => Self::git_tint(&theme, status),
                    None if is_selected => theme.fg0,
                    None => theme.fg1,
                };
                let chevron = if row.is_dir {
                    if row.is_expanded { "▾" } else { "▸" }
                } else {
                    ""
                };
                div()
                    .id(("tree-row", index))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(ROW_HEIGHT))
                    .pr_2()
                    .pl(px(6.0 + row.depth as f32 * INDENT))
                    .text_size(px(12.5))
                    .text_color(if is_selected { theme.fg0 } else { theme.fg1 })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .when(is_selected, |element| {
                        element.bg(theme.bg3).border_l_2().border_color(color)
                    })
                    // gitignore 対象は淡く（git 管理外が一目で分かる）。
                    .when(row.ignored, |element| element.opacity(0.45))
                    // ファイルはチャット composer へドラッグ → @メンション参照にできる。
                    .when(!is_dir, |element| {
                        let mention =
                            row.path.strip_prefix(&root).unwrap_or(&row.path).to_string_lossy().to_string();
                        let theme = theme.clone();
                        element.on_drag(
                            DraggedFile { path: mention.into(), theme },
                            |dragged, _offset, _window, cx| cx.new(|_| dragged.clone()),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(px(9.))
                            .text_size(px(9.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(chevron.to_string())),
                    )
                    .child(file_icon(&row.name, is_dir, &theme))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(name_color)
                            .child(row.name.clone()),
                    )
                    // git バッジ（ファイル=状態文字・フォルダ=変更あり ●）
                    .when_some(file_status, |element, status| {
                        element.child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(Self::git_tint(&theme, status))
                                .child(Self::git_letter(status)),
                        )
                    })
                    .when(dir_dirty, |element| {
                        element.child(
                            div().flex_none().text_size(px(10.)).text_color(theme.warn).child("●"),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if is_dir {
                                this.toggle_dir(path.clone(), cx);
                            } else {
                                this.open_file(path.clone(), window, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener({
                            let path = row.path.clone();
                            move |this, event: &MouseDownEvent, _window, cx| {
                                this.show_context_menu(path.clone(), is_dir, event.position, cx)
                            }
                        }),
                    )
            }))
            .into_any_element()
    }

    /// アイコングリッド表示（現在フォルダの直下。フォルダはクリックで中に入る・ファイルは開く）。
    fn render_icons(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let dir = slot.current_dir.clone().unwrap_or_else(|| slot.worktree.root().to_path_buf());
        let entries = slot.worktree.read_any_dir(&dir).unwrap_or_default();
        let selected = slot.selected.clone();
        div()
            .flex_1()
            .overflow_hidden()
            .flex()
            .flex_wrap()
            .content_start()
            .gap(px(2.))
            .p(px(6.))
            .children(entries.into_iter().enumerate().map(|(index, entry)| {
                let is_dir = entry.is_dir;
                let is_ignored = entry.ignored;
                let path = entry.path.clone();
                let is_selected = selected.as_ref() == Some(&entry.path);
                div()
                    .id(("icon-cell", index))
                    .w(px(84.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .px(px(4.))
                    .py(px(8.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    .when(is_selected, |element| element.bg(theme.bg3))
                    .when(is_ignored, |element| element.opacity(0.45))
                    // 大きめアイコン（グリッド用に 2 倍スケール）
                    .child(div().flex().items_center().justify_center().h(px(30.)).child(icon_large(&entry.name, is_dir, &theme)))
                    .child(
                        div()
                            .max_w_full()
                            .text_size(px(11.))
                            .text_color(theme.fg1)
                            .text_center()
                            .overflow_hidden()
                            .child(SharedString::from(entry.name.clone())),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if is_dir {
                                this.enter_dir(path.clone(), cx);
                            } else {
                                this.open_file(path.clone(), window, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener({
                            let path = entry.path.clone();
                            move |this, event: &MouseDownEvent, _window, cx| {
                                this.show_context_menu(path.clone(), is_dir, event.position, cx)
                            }
                        }),
                    )
            }))
            .into_any_element()
    }

    /// Finder のカラム表示（Miller columns）。ルート→現在フォルダの各段をカラムで並べる。
    fn render_columns(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let root = slot.worktree.root().to_path_buf();
        let current = slot.current_dir.clone().unwrap_or_else(|| root.clone());
        // ルート → current の連鎖（各段がカラムになる）。
        let mut chain: Vec<PathBuf> = Vec::new();
        let mut walk = current.as_path();
        loop {
            chain.push(walk.to_path_buf());
            if walk == root {
                break;
            }
            match walk.parent() {
                Some(parent) if parent.starts_with(&root) || parent == root => walk = parent,
                _ => break,
            }
        }
        chain.reverse();
        // 460px に収まるよう末尾 3 段（＝現在フォルダ + 親 2 つ）だけ見せる。
        let visible_start = chain.len().saturating_sub(3);

        div()
            .flex_1()
            .flex()
            .overflow_hidden()
            .children(chain.iter().enumerate().skip(visible_start).map(|(column_index, dir)| {
                let entries = slot.worktree.read_any_dir(dir).unwrap_or_default();
                // このカラムで選択中（＝連鎖の次の段）のパス。
                let selected_child = chain.get(column_index + 1).cloned();
                div()
                    .w(px(150.))
                    .flex_none()
                    .h_full()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(theme.border)
                    .children(entries.into_iter().enumerate().map(|(row_index, entry)| {
                        let is_dir = entry.is_dir;
                        let is_ignored = entry.ignored;
                        let path = entry.path.clone();
                        let on_path = selected_child.as_ref() == Some(&entry.path);
                        div()
                            .id(("col", column_index * 1000 + row_index))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .h(px(ROW_HEIGHT))
                            .px(px(7.))
                            .text_size(px(12.))
                            .text_color(if on_path { theme.fg0 } else { theme.fg1 })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                            .when(on_path, |element| element.bg(theme.bg3))
                            .when(is_ignored, |element| element.opacity(0.45))
                            .child(file_icon(&entry.name, is_dir, &theme))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(SharedString::from(entry.name.clone())),
                            )
                            // フォルダは中に入る合図の ›
                            .when(is_dir, |element| {
                                element.child(div().flex_none().text_size(px(9.)).text_color(theme.fg2).child("›"))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    if is_dir {
                                        this.enter_dir(path.clone(), cx);
                                    } else {
                                        this.open_file(path.clone(), window, cx);
                                    }
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener({
                                    let path = entry.path.clone();
                                    move |this, event: &MouseDownEvent, _window, cx| {
                                        this.show_context_menu(path.clone(), is_dir, event.position, cx)
                                    }
                                }),
                            )
                    }))
            }))
            .into_any_element()
    }

    /// エクスプローラ上部のブレッドクラム。左の **⤴** で 1 段上へ（プロジェクトルートより上＝
    /// 隣のリポジトリへも辿れる。M5 受入）。プロジェクト配下では「プロジェクト名 → 各段」を、
    /// ルート外では「⌂プロジェクト（戻る） → 末尾数段」を出す。各段クリックでそのフォルダへ。
    fn render_explorer_header(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = slot.color;
        let root = slot.worktree.root().to_path_buf();
        let current = slot.current_dir.clone().unwrap_or_else(|| root.clone());
        let in_project = current.starts_with(&root);

        let mut header = div()
            .flex()
            .items_center()
            .gap(px(1.))
            .h(px(28.))
            .px(px(6.))
            .flex_none()
            .text_size(px(11.))
            .text_color(theme.fg2)
            .overflow_hidden()
            .border_b_1()
            .border_color(theme.border);

        // 「上へ」= current の親へ（ルート直上へ出れば enter_dir が Finder カラムへ自動切替）。
        if let Some(parent) = current.parent().map(Path::to_path_buf) {
            header = header.child(
                div()
                    .id("crumb-up")
                    .flex_none()
                    .px(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child("⤴")
                    .tooltip(Tooltip::text("上のフォルダへ", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.enter_dir(parent.clone(), cx)),
                    ),
            );
        }

        if in_project {
            // プロジェクト名（アクセント色・クリックでルートへ）→ 配下の各段。
            let root_for_click = root.clone();
            header = header.child(
                div()
                    .id(("crumb", 0usize))
                    .px(px(3.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .hover(|style| style.bg(theme.bg3))
                    .child(slot.name.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.enter_dir(root_for_click.clone(), cx)),
                    ),
            );
            if let Ok(relative) = current.strip_prefix(&root) {
                let segments: Vec<_> = relative.components().collect();
                let last = segments.len().saturating_sub(1);
                let mut accumulated = root.clone();
                for (index, segment) in segments.into_iter().enumerate() {
                    accumulated = accumulated.join(segment.as_os_str());
                    let is_current = index == last;
                    let label = segment.as_os_str().to_string_lossy().to_string();
                    let path = accumulated.clone();
                    header = header
                        .child(div().px(px(1.)).text_color(theme.fg2).child("›"))
                        .child(
                            div()
                                .id(("crumb", index + 1))
                                .px(px(3.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .when(is_current, |element| {
                                    element.text_color(theme.fg0).font_weight(FontWeight::SEMIBOLD)
                                })
                                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                                .child(SharedString::from(label))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| this.enter_dir(path.clone(), cx)),
                                ),
                        );
                }
            }
        } else {
            // ルート外ブラウズ: ⌂プロジェクト（戻る）+ current までの末尾最大 3 段。
            let root_for_click = root.clone();
            header = header.child(
                div()
                    .id("crumb-home")
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .px(px(4.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .hover(|style| style.bg(theme.bg3))
                    .child("⌂")
                    .child(slot.name.clone())
                    .tooltip(Tooltip::text("プロジェクトへ戻る", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.enter_dir(root_for_click.clone(), cx)),
                    ),
            );
            let mut chain: Vec<PathBuf> = Vec::new();
            let mut walk = Some(current.as_path());
            while let Some(path) = walk {
                chain.push(path.to_path_buf());
                walk = path.parent();
                if chain.len() >= 3 {
                    break;
                }
            }
            chain.reverse();
            let last = chain.len().saturating_sub(1);
            for (index, path) in chain.into_iter().enumerate() {
                let is_current = index == last;
                let label = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                header = header
                    .child(div().px(px(1.)).text_color(theme.fg2).child("›"))
                    .child(
                        div()
                            .id(("crumb-out", index))
                            .px(px(3.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .when(is_current, |element| {
                                element.text_color(theme.fg0).font_weight(FontWeight::SEMIBOLD)
                            })
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                            .child(SharedString::from(label))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| this.enter_dir(path.clone(), cx)),
                            ),
                    );
            }
        }
        header
    }

    /// エクスプローラ下部の表示モード切替（ツリー / カラム / アイコン）。
    fn render_explorer_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let current = self.explorer_view;
        let button = |view: ExplorerView, id: &'static str, glyph: &'static str, tip: &'static str| {
            let active = current == view;
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(px(5.))
                .text_size(px(13.))
                .text_color(if active { theme.fg0 } else { theme.fg2 })
                .cursor_pointer()
                .when(active, |element| element.bg(theme.bg3))
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(glyph)
                .tooltip(Tooltip::text(tip, theme.clone()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| this.set_explorer_view(view, cx)),
                )
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(30.))
            .px(px(6.))
            .flex_none()
            .border_t_1()
            .border_color(theme.border)
            .child(button(ExplorerView::Tree, "view-tree", "☰", "ツリー表示"))
            .child(button(ExplorerView::Columns, "view-columns", "▥", "カラム表示（Finder 風）"))
            .child(button(ExplorerView::Icons, "view-icons", "▦", "アイコン表示"))
    }

    /// エクスプローラの右クリックメニュー（開いていれば）。フォルダ=新規ウィンドウで開く 等。
    /// 背後に透明バックドロップを敷き、外側クリックで閉じる。
    fn render_explorer_context_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.explorer_context_menu.as_ref()?;
        let (bg2, bg3, border, fg1, fg0) =
            (self.theme.bg2, self.theme.bg3, self.theme.border, self.theme.fg1, self.theme.fg0);
        let path = menu.path.clone();
        let is_dir = menu.is_dir;
        let position = menu.position;

        let item = move |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .px(px(9.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(fg1)
                .cursor_pointer()
                .hover(move |style| style.bg(bg3).text_color(fg0))
                .child(label)
        };

        let mut menu_box = div()
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(210.))
            .bg(bg2)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
            ]);

        if is_dir {
            let open_path = path.clone();
            menu_box = menu_box.child(item("ctx-open-window", "新規ウィンドウで開く").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.open_folder_as_window(open_path.clone(), cx)),
            ));
            let enter_path = path.clone();
            menu_box = menu_box.child(item("ctx-enter", "このフォルダを開く").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.enter_dir(enter_path.clone(), cx);
                    this.hide_context_menu(cx);
                }),
            ));
        } else {
            let open_path = path.clone();
            menu_box = menu_box.child(item("ctx-open", "開く").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.open_file(open_path.clone(), window, cx);
                    this.hide_context_menu(cx);
                }),
            ));
        }
        let copy_path = path.clone();
        menu_box = menu_box.child(item("ctx-copy", "パスをコピー").on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _window, cx| this.copy_path(&copy_path, cx)),
        ));

        // 透明バックドロップ（外側クリックで閉じる）。メニューはその子（最前面）。
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.hide_context_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _window, cx| this.hide_context_menu(cx)),
                )
                .child(menu_box)
                .into_any_element(),
        )
    }

    /// プロジェクト横断検索パネル（オーバーレイ・⌘⇧F）。ファイル別に結果をまとめ、クリック/Enter でジャンプ。
    fn render_search_panel(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.search_panel.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let root = self.active_slot().map(|slot| slot.worktree.root().to_path_buf());

        let query_display: SharedString = if state.query.is_empty() {
            SharedString::from("プロジェクトを横断検索…")
        } else {
            SharedString::from(state.query.clone())
        };
        let query_color = if state.query.is_empty() { theme.fg2 } else { theme.fg0 };

        // トグルチップ（Aa=大小区別 / .*=正規表現）。アクティブはアクセント面。
        let case_active = state.case_sensitive;
        let regex_active = state.is_regex;
        let toggle = |id: &'static str, label: &'static str, active: bool, tip: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(5.))
                .text_size(px(11.))
                .text_color(if active { theme.fg0 } else { theme.fg2 })
                .cursor_pointer()
                .when(active, |element| element.bg(accent.alpha(0.16)))
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(label)
                .tooltip(Tooltip::text(tip, theme.clone()))
        };

        // 見出し（件数 / エラー / ヒント）。
        let summary: SharedString = match &state.error {
            _ if state.running => SharedString::from("検索中…"),
            Some(error) => error.clone(),
            None if state.results_query.is_some() => {
                SharedString::from(format!("{} 件 / {} ファイル", state.total_matches(), state.results.len()))
            }
            None => SharedString::from("Enter で検索"),
        };
        let summary_color = if state.error.is_some() { theme.err } else { theme.fg2 };

        // 結果行（ファイルヘッダ + マッチ）。平坦選択と対応させる。
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let mut flat_index = 0usize;
        let mut truncated = false;
        'outer: for (file_index, file) in state.results.iter().enumerate() {
            let relative = root
                .as_ref()
                .and_then(|root| file.path.strip_prefix(root).ok())
                .unwrap_or(file.path.as_path())
                .to_string_lossy()
                .to_string();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .pt(px(7.))
                    .pb(px(2.))
                    .child(file_icon(&relative, false, &theme))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg1)
                            .child(SharedString::from(relative)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(file.matches.len().to_string())),
                    )
                    .into_any_element(),
            );
            for (match_index, found) in file.matches.iter().enumerate() {
                if flat_index >= SEARCH_MAX_ROWS {
                    truncated = true;
                    break 'outer;
                }
                let is_selected = flat_index == state.selected;
                // 行プレビュー: 先頭空白を除いて（深いインデントでマッチが見切れないように）、
                // マッチ部分をアクセント色で強調する。オフセットは byte 境界（regex が行内で見つけた位置）。
                let raw = found.line_text.trim_end();
                let lead = raw.len() - raw.trim_start().len();
                let line = &raw[lead..];
                let start = found.column.saturating_sub(lead).min(line.len());
                let end = (found.column + found.byte_range.len()).saturating_sub(lead).clamp(start, line.len());
                let (prefix, mid, suffix) = (&line[..start], &line[start..end], &line[end..]);
                rows.push(
                    div()
                        .id(("search-hit", flat_index))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(20.))
                        .pl(px(28.))
                        .pr(px(10.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(is_selected, |element| element.bg(accent.alpha(0.16)))
                        .hover(|style| style.bg(theme.bg3))
                        .child(
                            div()
                                .flex_none()
                                .w(px(34.))
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from((found.line + 1).to_string())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.))
                                .text_color(theme.fg1)
                                .child(div().flex_none().child(SharedString::from(prefix.to_string())))
                                .child(
                                    div()
                                        .flex_none()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(accent)
                                        .child(SharedString::from(mid.to_string())),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .overflow_hidden()
                                        .child(SharedString::from(suffix.to_string())),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.jump_to_search_match(file_index, match_index, window, cx)
                            }),
                        )
                        .into_any_element(),
                );
                flat_index += 1;
            }
        }
        if truncated {
            rows.push(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(format!("先頭 {SEARCH_MAX_ROWS} 件のみ表示（絞り込んでください）")))
                    .into_any_element(),
            );
        }

        let focus = state.focus.clone();
        Some(
            div()
                .absolute()
                .inset_0()
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_search_key_down))
                // 背後クリックで閉じる（中央のボックスは stop_propagation）。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_search(window, cx)),
                )
                .flex()
                .flex_col()
                .items_center()
                .pt(px(96.))
                .child(
                    div()
                        .w(px(720.))
                        .flex()
                        .flex_col()
                        .bg(theme.bg2)
                        .rounded(px(12.))
                        .border_1()
                        .border_color(theme.border)
                        .shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(10.), gpui::hsla(0., 0., 0., 0.45)).blur_radius(px(28.)),
                        ])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        // 入力行（クエリ + トグル）
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(theme.border)
                                .child(div().flex_none().text_color(theme.fg2).child("⌕"))
                                .child(div().flex_1().text_color(query_color).child(query_display))
                                .child(toggle("search-case", "Aa", case_active, "大文字小文字を区別").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.toggle_search_case(cx)
                                    }),
                                ))
                                .child(toggle("search-regex", ".*", regex_active, "正規表現").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.toggle_search_regex(cx)
                                    }),
                                )),
                        )
                        // 見出し（件数 / エラー / ヒント）
                        .child(
                            div()
                                .px_3()
                                .py(px(4.))
                                .text_size(px(11.))
                                .text_color(summary_color)
                                .child(summary),
                        )
                        // 結果リスト
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .max_h(px(440.))
                                .overflow_hidden()
                                .pb_1()
                                .children(rows),
                        )
                        // フッタのキーヒント
                        .child(
                            div()
                                .flex()
                                .gap(px(12.))
                                .px_3()
                                .py(px(5.))
                                .border_t_1()
                                .border_color(theme.border)
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child("Enter 検索 / ジャンプ")
                                .child("↑↓ 選択")
                                .child("Esc 閉じる"),
                        ),
                )
                .into_any_element(),
        )
    }

    /// LSP 補完ポップアップ（Ctrl-Space）。キャレット直下に候補リスト。上下/Enter・Tab/Esc で操作。
    fn render_completion(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.completion.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let focus = state.focus.clone();
        let selected = state.selected;

        let list = div().flex().flex_col().max_h(px(260.)).overflow_hidden().children(
            state.items.iter().take(12).enumerate().map(|(row, item)| {
                let is_selected = row == selected;
                div()
                    .id(("completion", row))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(is_selected, |element| element.bg(accent.alpha(0.16)))
                    .hover(|style| style.bg(theme.bg3))
                    .child(
                        div()
                            .flex_none()
                            .w(px(34.))
                            .text_size(px(10.))
                            .text_color(accent)
                            .child(item.kind.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.5))
                            .text_color(if is_selected { theme.fg0 } else { theme.fg1 })
                            .child(item.label.clone()),
                    )
                    .when_some(item.detail.clone(), |element, detail| {
                        element.child(
                            div()
                                .flex_none()
                                .max_w(px(150.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(detail),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if let Some(state) = this.completion.as_mut() {
                                state.selected = row;
                            }
                            this.confirm_completion(window, cx)
                        }),
                    )
            }),
        );

        Some(
            div()
                .absolute()
                .inset_0()
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_completion_key_down))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_completion(window, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .left(state.position.x)
                        .top(state.position.y + px(2.))
                        .w(px(360.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .p(px(4.))
                        .shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
                        ])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(list),
                )
                .into_any_element(),
        )
    }

    /// branch/worktree メニュー（titlebar の ⎇ クリックで開く）。ブランチ切替（in-place）と、
    /// **worktree を別ウィンドウで開く**（並行ブランチ×別窓×スレッド色＝当初ビジョン）。
    /// git 操作パネル（ソース管理・M8）。左カラムでエクスプローラと切り替えて出す。
    /// 変更一覧（staged/unstaged）・コミット・push/pull・新規ブランチを 1 面にまとめる。
    fn render_git_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let (bg0, bg1, bg2, border, fg0, fg1, fg2) =
            (theme.bg0, theme.bg1, theme.bg2, theme.border, theme.fg0, theme.fg1, theme.fg2);
        let Some(state) = self.git_panel.as_ref() else {
            return div().w(px(self.explorer_width)).h_full().flex_none().bg(bg1).into_any_element();
        };
        let focus = state.focus.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        if self.active_slot().is_none() {
            return div()
                .w(px(self.explorer_width))
                .h_full()
                .flex_none()
                .bg(bg1)
                .border_r_1()
                .border_color(border)
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_git_key_down))
                .child(div().p(px(12.)).text_size(px(11.5)).text_color(fg2).child("プロジェクトがありません"))
                .into_any_element();
        }
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());
        let changes = &self.git_changes;
        let naming = state.branch_name.is_some();

        // ── ヘッダ（タイトル + ブランチ + ＋ + ×）──
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(34.))
            .px(px(10.))
            .flex_none()
            .bg(bg0)
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg1)
                    .child("ソース管理"),
            )
            .child(div().flex_1())
            .when_some(branch.clone(), |element, branch| {
                element.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .max_w(px(120.))
                        .child(div().flex_none().text_size(px(10.5)).text_color(accent).child("⎇"))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(11.))
                                .text_color(fg2)
                                .child(SharedString::from(branch)),
                        ),
                )
            })
            // GitHub 連携（origin が GitHub のときだけ。PR 作成 / PR・リポジトリを開く）
            .when(self.github_slug.is_some(), |element| {
                element
                    .child(
                        div()
                            .id("git-pr-create")
                            .flex_none()
                            .px(px(5.))
                            .rounded(px(4.))
                            .text_size(px(10.5))
                            .text_color(fg2)
                            .cursor_pointer()
                            .hover(|style| style.bg(bg2).text_color(fg0))
                            .child("PR")
                            .tooltip(Tooltip::text("GitHub で PR を作成（gh pr create --web）", theme.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| this.github_action(true, window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("git-pr-open")
                            .flex_none()
                            .px(px(5.))
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .text_color(fg2)
                            .cursor_pointer()
                            .hover(|style| style.bg(bg2).text_color(fg0))
                            .child("↗")
                            .tooltip(Tooltip::text("GitHub で PR / リポジトリを開く", theme.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| this.github_action(false, window, cx)),
                            ),
                    )
            })
            .child(
                div()
                    .id("git-new-branch")
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(bg2).text_color(fg0))
                    .child("＋")
                    .tooltip(Tooltip::text("新しいブランチ", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.start_new_branch(window, cx)),
                    ),
            )
            .child(
                div()
                    .id("git-close")
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(bg2).text_color(fg0))
                    .child("×")
                    .tooltip(Tooltip::text("閉じる  ⌃⇧G", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.toggle_git_panel(&ToggleGitPanel, window, cx)
                        }),
                    ),
            );

        // ── 入力行（コミットメッセージ / ブランチ名）──
        let input_text = match &state.branch_name {
            Some(name) => name.clone(),
            None => state.message.clone(),
        };
        let placeholder = if naming {
            "新しいブランチ名（⏎ で作成・Esc で取消）"
        } else {
            "メッセージ（⌘⏎ でコミット）"
        };
        let input_body = if input_text.is_empty() {
            div().text_color(fg2).child(SharedString::from(placeholder))
        } else {
            div().text_color(fg0).child(SharedString::from(format!("{input_text}▍")))
        };
        let input_row = div()
            .id("git-input")
            .m(px(8.))
            .p(px(8.))
            .h(px(46.))
            .bg(bg2)
            .border_1()
            .border_color(if naming { accent } else { border })
            .rounded(px(6.))
            .text_size(px(12.))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if let Some(state) = this.git_panel.as_ref() {
                        let focus = state.focus.clone();
                        window.focus(&focus, cx);
                    }
                }),
            )
            .child(input_body);

        // ── アクション行（commit / push / pull）。ブランチ名モードでは出さない ──
        let commit_ready = !state.message.trim().is_empty();
        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .pb(px(8.))
            .flex_none()
            // ✨ AI でコミットメッセージ生成（Claude Code CLI に diff を渡す）
            .child(
                div()
                    .id("git-ai-message")
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(26.))
                    .h(px(26.))
                    .rounded(px(6.))
                    .bg(bg2)
                    .text_size(px(13.))
                    .text_color(if self.git_busy { fg2 } else { fg1 })
                    .when(!self.git_busy, |element| {
                        element.cursor_pointer().hover(|style| style.bg(theme.bg3).text_color(fg0)).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.generate_commit_message(window, cx)),
                        )
                    })
                    .child("✨")
                    .tooltip(Tooltip::text("AI でメッセージ生成（claude）", theme.clone())),
            )
            .child(
                div()
                    .id("git-commit")
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(26.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .when(commit_ready, |element| {
                        element.bg(accent).text_color(theme.bg0).cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.git_commit(window, cx)),
                        )
                    })
                    .when(!commit_ready, |element| element.bg(bg2).text_color(fg2))
                    .child("✓ コミット"),
            )
            .child(self.git_remote_button("git-push", "↑", "push", true, cx))
            .child(self.git_remote_button("git-pull", "↓", "pull", false, cx))
            .when(self.git_busy, |element| {
                element.child(div().text_size(px(11.)).text_color(fg2).child("…"))
            });

        // ── 変更一覧（staged / unstaged）。高さを抑えて下に履歴を置く ──
        let mut body = div().flex_none().max_h(px(240.)).flex().flex_col().overflow_hidden().pb(px(6.));
        let staged_count = changes.iter().filter(|change| change.staged.is_some()).count();
        let unstaged_count = changes.iter().filter(|change| change.unstaged.is_some()).count();
        if staged_count > 0 {
            body = body.child(self.git_section_header("ステージ済み", staged_count, false, cx));
            for (index, change) in changes.iter().filter(|change| change.staged.is_some()).enumerate() {
                if let Some(kind) = change.staged {
                    body = body.child(self.git_change_row(change.path.clone(), kind, true, index, cx));
                }
            }
        }
        if unstaged_count > 0 {
            body = body.child(self.git_section_header("変更", unstaged_count, true, cx));
            for (index, change) in changes.iter().filter(|change| change.unstaged.is_some()).enumerate() {
                if let Some(kind) = change.unstaged {
                    body = body.child(self.git_change_row(change.path.clone(), kind, false, index, cx));
                }
            }
        }
        if staged_count == 0 && unstaged_count == 0 {
            body = body.child(
                div().px(px(12.)).py(px(8.)).text_size(px(11.5)).text_color(fg2).child("変更はありません"),
            );
        }

        div()
            .w(px(self.explorer_width))
            .h_full()
            .flex()
            .flex_col()
            .flex_none()
            .bg(bg1)
            .border_r_1()
            .border_color(border)
            .track_focus(&focus)
            .on_key_down(cx.listener(Self::on_git_key_down))
            .child(header)
            .child(input_row)
            .when(!naming, |element| element.child(actions))
            .child(body)
            .child(div().h(px(1.)).flex_none().bg(border))
            .child(self.render_git_history(&self.git_history))
            .into_any_element()
    }

    /// push/pull の丸ボタン（背景実行中は無効表示）。
    fn git_remote_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: &'static str,
        is_push: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let disabled = self.git_busy;
        div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(26.))
            .h(px(26.))
            .rounded(px(6.))
            .bg(theme.bg2)
            .text_size(px(13.))
            .text_color(if disabled { theme.fg2 } else { theme.fg1 })
            .when(!disabled, |element| {
                element.cursor_pointer().hover(|style| style.bg(theme.bg3).text_color(theme.fg0)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if is_push {
                            this.git_push(window, cx)
                        } else {
                            this.git_pull(window, cx)
                        }
                    }),
                )
            })
            .child(glyph)
            .tooltip(Tooltip::text(label, theme.clone()))
    }

    /// 変更一覧のセクション見出し（"ステージ済み"/"変更" + 件数。"変更"側は「すべてステージ」付き）。
    fn git_section_header(
        &self,
        title: &'static str,
        count: usize,
        stage_all: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(3.))
            .pt(px(6.))
            .flex_none()
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg2)
                    .child(format!("{title}  {count}")),
            )
            .child(div().flex_1())
            .when(stage_all, |element| {
                element.child(
                    div()
                        .id("git-stage-all")
                        .px(px(5.))
                        .rounded(px(4.))
                        .text_size(px(13.))
                        .text_color(theme.fg2)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                        .child("＋")
                        .tooltip(Tooltip::text("すべてステージ", theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.git_stage_all(cx)),
                        ),
                )
            })
    }

    /// 変更 1 行（色付きレター + ファイル名 + stage/unstage ボタン）。
    fn git_change_row(
        &self,
        path: PathBuf,
        kind: StatusKind,
        staged: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let tint = Self::git_tint(&theme, kind);
        let letter = Self::git_letter(kind);
        let name =
            path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let row_id = if staged { "git-staged" } else { "git-unstaged" };
        let act_id = if staged { "git-unstage" } else { "git-stage" };
        let action_path = path.clone();
        div()
            .id((row_id, index))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(3.))
            .text_size(px(12.))
            .hover(|style| style.bg(theme.bg3))
            .child(div().w(px(12.)).flex_none().text_size(px(11.)).text_color(tint).child(letter))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme.fg1)
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .id((act_id, index))
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                    .child(if staged { "−" } else { "＋" })
                    .tooltip(Tooltip::text(
                        if staged { "ステージ解除" } else { "ステージ" },
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            if staged {
                                this.git_unstage(action_path.clone(), cx)
                            } else {
                                this.git_stage(action_path.clone(), cx)
                            }
                        }),
                    ),
            )
            .into_any_element()
    }

    /// git 履歴（コミットグラフ・M8）。レーン線＝矩形（railway）で描き、色はプロジェクト色
    /// パレットに乗せる（ブランチ＝色。「色による方向感覚」）。点/線/コネクタは絶対配置の div。
    fn render_git_history(&self, commits: &[GraphCommit]) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        // 描画幅のための最大レーン。
        let max_lane = commits
            .iter()
            .map(|commit| {
                commit
                    .lanes_in
                    .iter()
                    .chain(&commit.lanes_out)
                    .chain(&commit.connectors)
                    .copied()
                    .fold(commit.dot_lane, usize::max)
            })
            .max()
            .unwrap_or(0);
        let lane_w = 14.0_f32;
        let row_h = 22.0_f32;
        let mid = row_h / 2.0;
        let thick = 1.5_f32;
        let dot_r = 3.0_f32;
        let cell_w = (max_lane as f32 + 1.0) * lane_w;
        let lane_x = |lane: usize| lane as f32 * lane_w + lane_w / 2.0;

        let mut list = div().flex().flex_col().flex_1().min_h_0().overflow_hidden().child(
            div()
                .flex_none()
                .px(px(10.))
                .py(px(3.))
                .pt(px(6.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg2)
                .child("履歴"),
        );

        for commit in commits {
            // ── グラフセル（点 + 縦レーン + 横コネクタ）──
            let mut cell = div().relative().flex_none().h(px(row_h)).w(px(cell_w));
            for &lane in &commit.lanes_in {
                cell = cell.child(
                    div()
                        .absolute()
                        .left(px(lane_x(lane) - thick / 2.0))
                        .top(px(0.))
                        .w(px(thick))
                        .h(px(mid))
                        .bg(project_color(lane)),
                );
            }
            for &lane in &commit.lanes_out {
                cell = cell.child(
                    div()
                        .absolute()
                        .left(px(lane_x(lane) - thick / 2.0))
                        .top(px(mid))
                        .w(px(thick))
                        .h(px(row_h - mid))
                        .bg(project_color(lane)),
                );
            }
            for &lane in &commit.connectors {
                let start = lane_x(commit.dot_lane).min(lane_x(lane));
                let end = lane_x(commit.dot_lane).max(lane_x(lane));
                cell = cell.child(
                    div()
                        .absolute()
                        .left(px(start))
                        .top(px(mid - thick / 2.0))
                        .w(px(end - start))
                        .h(px(thick))
                        .bg(project_color(commit.dot_lane)),
                );
            }
            cell = cell.child(
                div()
                    .absolute()
                    .left(px(lane_x(commit.dot_lane) - dot_r))
                    .top(px(mid - dot_r))
                    .w(px(dot_r * 2.0))
                    .h(px(dot_r * 2.0))
                    .rounded(px(dot_r))
                    .bg(project_color(commit.dot_lane)),
            );

            // ── テキスト（ref チップ + 要約 + hash）──
            let mut text =
                div().flex_1().flex().items_center().gap(px(6.)).overflow_hidden().whitespace_nowrap();
            for reference in &commit.refs {
                let is_head = reference.contains("HEAD");
                let label = reference.trim_start_matches("HEAD -> ").to_string();
                text = text.child(
                    div()
                        .flex_none()
                        .px(px(4.))
                        .rounded(px(3.))
                        .text_size(px(9.5))
                        .bg(theme.bg2)
                        .text_color(if is_head { accent } else { theme.fg2 })
                        .child(SharedString::from(label)),
                );
            }
            text = text
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .child(SharedString::from(commit.summary.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(commit.short_hash.clone())),
                );

            list = list.child(
                div().flex().items_center().gap(px(6.)).px(px(10.)).h(px(row_h)).child(cell).child(text),
            );
        }
        if commits.is_empty() {
            list = list.child(
                div().px(px(12.)).py(px(6.)).text_size(px(11.)).text_color(theme.fg2).child("コミットがありません"),
            );
        }
        list
    }

    fn render_branch_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.branch_menu.as_ref()?;
        let position = menu.position;
        let slot = self.active_slot()?;
        let theme = self.theme.clone();
        let accent = slot.color;
        let current = menu.current.clone();
        let branches = menu.branches.clone();
        let worktrees = menu.worktrees.clone();

        let (bg2, bg3, border, fg0, fg1, fg2) =
            (theme.bg2, theme.bg3, theme.border, theme.fg0, theme.fg1, theme.fg2);

        let mut menu_box = div()
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(280.))
            .bg(bg2)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
            ])
            .child(div().px(px(8.)).py(px(4.)).text_size(px(10.5)).text_color(fg2).child("ブランチ"));

        for (index, branch) in branches.into_iter().enumerate() {
            let is_current = current.as_deref() == Some(branch.as_str());
            let switch_branch = branch.clone();
            let worktree_branch = branch.clone();
            let delete_branch_name = branch.clone();
            menu_box = menu_box.child(
                div()
                    .id(("branch", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|style| style.bg(bg3))
                    .child(
                        div()
                            .w(px(10.))
                            .flex_none()
                            .text_color(accent)
                            .child(if is_current { "●" } else { "" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(if is_current { fg0 } else { fg1 })
                            .child(SharedString::from(branch.clone())),
                    )
                    // 行クリック = in-place 切替（現在ブランチは無効）
                    .when(!is_current, |element| {
                        element.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.switch_branch_to(switch_branch.clone(), window, cx)
                            }),
                        )
                    })
                    // ⧉ = worktree として新しい窓で開く（当初ビジョン）
                    .child(
                        div()
                            .id(("branch-wt", index))
                            .flex_none()
                            .px(px(4.))
                            .rounded(px(4.))
                            .text_size(px(11.))
                            .text_color(fg2)
                            .hover(|style| style.bg(bg2).text_color(fg0))
                            .child("⧉")
                            .tooltip(Tooltip::text("worktree で新しい窓に開く", theme.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.open_branch_worktree(worktree_branch.clone(), cx)
                                }),
                            ),
                    )
                    // 🗑 = ブランチ削除（現在ブランチ以外。未マージは git が -d で拒否＝安全側）
                    .when(!is_current, |element| {
                        let delete_branch_name = delete_branch_name.clone();
                        element.child(
                            div()
                                .id(("branch-del", index))
                                .flex_none()
                                .px(px(4.))
                                .rounded(px(4.))
                                .text_size(px(11.))
                                .text_color(fg2)
                                .hover(|style| style.bg(bg2).text_color(theme.err))
                                .child("🗑")
                                .tooltip(Tooltip::text("ブランチを削除", theme.clone()))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.delete_git_branch(delete_branch_name.clone(), cx)
                                    }),
                                ),
                        )
                    }),
            );
        }

        // worktree セクション（現在の作業ツリー以外）。
        let root = slot.worktree.root();
        let others: Vec<_> = worktrees.into_iter().filter(|worktree| worktree.path != root).collect();
        if !others.is_empty() {
            menu_box = menu_box
                .child(div().h(px(1.)).bg(border).my(px(3.)))
                .child(div().px(px(8.)).py(px(4.)).text_size(px(10.5)).text_color(fg2).child("worktree"));
            for (index, worktree) in others.into_iter().enumerate() {
                let label = worktree.branch.clone().unwrap_or_else(|| {
                    worktree
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default()
                });
                let path = worktree.path.clone();
                menu_box = menu_box.child(
                    div()
                        .id(("worktree", index))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(5.))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|style| style.bg(bg3))
                        .child(div().flex_none().text_color(fg2).child("⎇"))
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(fg1)
                                .child(SharedString::from(label)),
                        )
                        .child(div().flex_none().text_size(px(10.)).text_color(fg2).child("窓"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.open_worktree_window(path.clone(), cx)
                            }),
                        ),
                );
            }
        }

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.hide_branch_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _window, cx| this.hide_branch_menu(cx)),
                )
                .child(menu_box)
                .into_any_element(),
        )
    }

    /// アクティブプロジェクト色（無ければパレット先頭）。titlebar ピル左縁・タブ上線に流す。
    fn accent(&self) -> Hsla {
        self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0))
    }

    // ── titlebar（UI-SPEC §3） ──

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .id("titlebar")
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .gap_2()
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .pl(px(TRAFFIC_LIGHT_INSET)) // ネイティブ信号機を避ける
            .pr_2()
            .border_b_1()
            .border_color(theme.border)
            // 窓ドラッグ: down→move で開始（クリックと区別）。ダブルクリックで zoom（Zed 準拠）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.should_move_window = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.should_move_window = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _window, _cx| this.should_move_window = false))
            .on_mouse_move(cx.listener(|this, _, window, _cx| {
                if this.should_move_window {
                    this.should_move_window = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _cx| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(self.render_project_pill(cx))
            .child(div().flex_1().h_full()) // 空き＝ドラッグ領域（titlebar 全体で処理）
            // 実行中スレッドの beacon（窓上部から常に見える＝方向感覚の核・UI-SPEC §3）
            .child(self.render_beacons(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(self.dock_button(Dock::Left, self.show_left, cx))
                    .child(self.dock_button(Dock::Bottom, self.show_bottom, cx))
                    .child(self.dock_button(Dock::Right, self.show_right, cx)),
            )
    }

    /// titlebar の beacon 列（アクティブプロジェクトのスレッド。実行中は色濃く・停止中は淡く）。
    fn render_beacons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let beacons = self.agent_panel.read(cx).beacons();
        div()
            .flex()
            .items_center()
            .gap_3()
            .mr_2()
            .children(beacons.into_iter().enumerate().map(|(index, (name, color, running))| {
                let status = if running { "実行中" } else { "待機中" };
                let label = format!("{name} — {status}");
                div()
                    .id(("beacon-item", index))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(px(11.))
                    .text_color(if running { theme.fg1 } else { theme.fg2 })
                    .child(beacon_dot(("beacon", index), color, running))
                    .child(name)
                    .tooltip(Tooltip::text(label, theme.clone()))
            }))
    }

    /// プロジェクトピル: 枠 + 左縁 3px プロジェクト色 + 「名前 ▾」+「⎇ branch」。名前クリックで ⌘O。
    fn render_project_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let name = self
            .active_slot()
            .map(|slot| slot.name.clone())
            .unwrap_or_else(|| SharedString::from("—"));
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());

        let mut inner = div().flex().items_center().gap(px(7.)).py(px(3.)).px(px(9.)).child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg0)
                .child(format!("{name} ▾")),
        );
        if let Some(branch) = branch {
            inner = inner.child(
                div()
                    .id("branch-pill")
                    .text_color(theme.fg1)
                    .text_size(px(11.5))
                    .rounded(px(4.))
                    .px(px(3.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                    .child(format!("⎇ {branch}"))
                    .tooltip(Tooltip::text("ブランチ / worktree", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation(); // ピル全体（⌘O）を起こさない
                            this.toggle_branch_menu(event.position, cx)
                        }),
                    ),
            );
        }

        div()
            .id("project-pill")
            .flex()
            .items_stretch()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|style| style.border_color(theme.fg2).bg(theme.bg1))
            .overflow_hidden()
            .text_size(px(12.))
            .child(div().w(px(3.)).bg(accent)) // 左縁＝プロジェクト色
            .child(inner)
            .tooltip(Tooltip::text("プロジェクト / ブランチを切替  ⌘O", theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation(); // titlebar ドラッグを起こさない
                    this.open_project_switcher(&ProjectSwitcher, window, cx)
                }),
            )
    }

    /// ドックトグルの小アイコン（枠 + 位置ストリップで左/右/下を示す）。
    fn dock_button(&self, dock: Dock, active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let strip = || div().bg(theme.fg1);
        let frame = div()
            .w(px(14.))
            .h(px(11.))
            .flex()
            .border_1()
            .border_color(theme.fg2)
            .rounded(px(2.))
            .overflow_hidden();
        let icon = match dock {
            Dock::Left => frame.child(strip().w(px(4.)).h_full()),
            Dock::Right => frame.justify_end().child(strip().w(px(4.)).h_full()),
            Dock::Bottom => frame.flex_col().justify_end().child(strip().h(px(3.5)).w_full()),
        };
        let (id, label): (&'static str, &'static str) = match dock {
            Dock::Left => ("dock-left", "左パネル（エクスプローラ）を表示/隠す"),
            Dock::Bottom => ("dock-bottom", "下パネル（ターミナル等）を表示/隠す"),
            Dock::Right => ("dock-right", "右パネル（エージェント）を表示/隠す"),
        };
        div()
            .id(id)
            .w(px(26.))
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .when(active, |element| element.bg(theme.bg3))
            .hover(|style| style.bg(theme.bg3))
            .child(icon)
            .tooltip(Tooltip::text(label, theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation(); // titlebar ドラッグを起こさない
                    // 下ドックはターミナル生成 + フォーカスを伴うので専用ハンドラへ。
                    if dock == Dock::Bottom {
                        this.toggle_terminal(&ToggleTerminal, window, cx);
                    } else {
                        this.toggle_dock(dock, cx);
                    }
                }),
            )
    }

    // ── タブ列・パンくず（UI-SPEC §5） ──

    fn render_tabstrip(
        &self,
        editor: &Entity<EditorView>,
        is_split: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let pane = is_split as usize; // ElementId を主/分割で一意にする
        let view = editor.read(cx);
        let name = view
            .buffer()
            .path()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "無題".to_string());
        let dirty = view.buffer().is_dirty();
        // タブ名も git 状態で色付け（ツリーと同じ色貫通）。
        let status = view.buffer().path().and_then(|path| self.git_status.get(path).copied());
        let name_color = status.map(|status| Self::git_tint(&theme, status)).unwrap_or(theme.fg0);
        // ファイルが変わったら（キーが変わる）タブが一度だけ fade-in する。
        let tab_key = SharedString::from(format!("editor-tab-appear-{pane}-{name}"));

        let tab = div()
            .id(("editor-tab", pane))
            .flex()
            .flex_col()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.bg1)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            // アクティブタブ上線 = プロジェクト色（UI-SPEC §5）
            .child(div().h(px(2.)).w_full().bg(accent))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(14.))
                    .text_size(px(12.))
                    .text_color(theme.fg0)
                    .when(dirty, |element| {
                        element.child(div().size(px(7.)).rounded(px(3.5)).bg(theme.warn))
                    })
                    .child(div().text_color(name_color).child(SharedString::from(name)))
                    .child(
                        div()
                            .id(("close-tab", pane))
                            .text_color(theme.fg2)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.fg0))
                            .child("×")
                            .tooltip(Tooltip::text(
                                if is_split { "分割を閉じる  ⌘\\" } else { "閉じる  ⌘W" },
                                theme.clone(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    if is_split {
                                        this.close_split(window, cx);
                                    } else {
                                        this.close_active_editor(cx);
                                    }
                                }),
                            ),
                    ),
            );

        div()
            .flex()
            .items_stretch()
            .h(px(TABSTRIP_HEIGHT))
            .flex_none()
            .bg(theme.bg0)
            .border_b_1()
            .border_color(theme.border)
            // 新しいファイルを開くとタブがすっと現れる（key=ファイル名。oneshot＝idle 0% 維持）。
            .child(tab.with_animation(
                tab_key,
                Animation::new(std::time::Duration::from_millis(200))
                    .with_easing(gpui::ease_out_quint()),
                |element, delta| element.opacity(delta),
            ))
            .child(div().flex_1())
    }

    fn render_breadcrumb(&self, editor: &Entity<EditorView>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let path = editor.read(cx).buffer().path().map(Path::to_path_buf);
        let root = self.active_slot().map(|slot| slot.worktree.root().to_path_buf());
        let crumbs = breadcrumb_text(root.as_deref(), path.as_deref());
        div()
            .flex()
            .items_center()
            .h(px(BREADCRUMB_HEIGHT))
            .px(px(14.))
            .flex_none()
            .bg(theme.bg1)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg2)
            .child(SharedString::from(crumbs))
    }

    /// 1 エディタペイン（タブ列 + パンくず + 本体）。分割時は左右で 2 枚並ぶ。
    fn render_editor_pane(
        &self,
        editor: &Entity<EditorView>,
        is_split: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(self.render_tabstrip(editor, is_split, cx))
            .child(self.render_breadcrumb(editor, cx))
            .child(div().flex_1().overflow_hidden().child(editor.clone()))
            .into_any_element()
    }

    fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let content = match self.editor.clone() {
            Some(editor) => {
                let mut panes = div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .child(self.render_editor_pane(&editor, false, cx));
                // 右分割ペイン（あれば仕切り + 2 枚目）。
                if let Some(split) = self.split_editor.clone() {
                    panes = panes
                        .child(div().w(px(1.)).flex_none().bg(theme.border))
                        .child(self.render_editor_pane(&split, true, cx));
                }
                panes.into_any_element()
            }
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("editor.empty_hint")))
                .into_any_element(),
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .bg(theme.bg1)
            .child(content)
            // 下ドック（ターミナル）はエディタ列の下に積む（サイドドックには被らない）。
            .when(self.show_bottom, |element| element.child(self.render_bottom_dock(cx)))
    }

    /// 下ドック（統合ターミナル・M8）。ヘッダ（タブ + 閉じる）+ ターミナル本体。
    fn render_bottom_dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let header = div()
            .flex()
            .items_center()
            .h(px(28.))
            .px(px(10.))
            .flex_none()
            .bg(theme.bg0)
            .border_t_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg1)
                    .child("ターミナル"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("term-close")
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0))
                    .child("×")
                    .tooltip(Tooltip::text("ターミナルを閉じる  ⌘J", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.toggle_terminal(&ToggleTerminal, window, cx)
                        }),
                    ),
            );
        let body = match &self.terminal {
            Some(terminal) => div().flex_1().min_h_0().overflow_hidden().child(terminal.clone()),
            None => div().flex_1(),
        };
        div()
            .h(px(BOTTOM_DOCK_HEIGHT))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .child(header)
            .child(body)
    }

    fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());
        let remote_host = self.active_slot().and_then(|slot| {
            slot.worktree
                .host()
                .is_remote()
                .then(|| SharedString::from(slot.worktree.host().display_name().to_string()))
        });
        let change_count = self.git_status.len();
        let (cursor, language) = match &self.editor {
            Some(editor) => {
                let view = editor.read(cx);
                let (row, column) = view.cursor_display();
                (Some(format!("{row}:{column}")), view.language_label())
            }
            None => (None, None),
        };

        let (errors, warnings) = self.active_diagnostic_counts(cx);
        let error_color = if errors > 0 { theme.err } else { theme.fg2 };
        let warning_color = if warnings > 0 { theme.warn } else { theme.fg2 };
        let left = div()
            .flex()
            .items_center()
            .gap_3()
            .when_some(remote_host, |element, remote_host| {
                let tooltip = format!("Remote SSH: {remote_host}");
                element.child(
                    div()
                        .id("statusbar-remote-host")
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .max_w(px(180.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(theme.fg0)
                        .child(div().text_color(theme.ok).child("SSH"))
                        .child(remote_host)
                        .tooltip(Tooltip::text(tooltip, theme.clone())),
                )
            })
            // ⎇ ブランチ + 変更件数バッジ。クリックで git パネル（ソース管理）を開閉。
            .when_some(branch, |element, branch| {
                let label = if change_count > 0 {
                    format!("⎇ {branch}  ●{change_count}")
                } else {
                    format!("⎇ {branch}")
                };
                element.child(
                    div()
                        .id("statusbar-branch")
                        .cursor_pointer()
                        .rounded(px(4.))
                        .px(px(4.))
                        .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                        .child(SharedString::from(label))
                        .tooltip(Tooltip::text("ソース管理  ⌃⇧G", theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.toggle_git_panel(&ToggleGitPanel, window, cx)
                            }),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(error_color).child(format!("✗ {errors}")))
                    .child(div().text_color(warning_color).child(format!("▲ {warnings}"))),
            );

        let right = div()
            .flex()
            .items_center()
            .gap_3()
            .when_some(cursor, |element, cursor| element.child(SharedString::from(cursor)))
            .child(SharedString::from("UTF-8"))
            .when_some(language, |element, language| element.child(language));

        div()
            .flex()
            .items_center()
            .h(px(STATUSBAR_HEIGHT))
            .px_3()
            .flex_none()
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .child(left)
            .child(div().flex_1())
            .child(right)
    }
}

/// 起動する言語サーバの記述子（コマンド + 引数 + LSP の languageId）。
struct LanguageServer {
    language_id: &'static str,
    command: PathBuf,
    args: Vec<&'static str>,
}

/// ファイルの拡張子から言語サーバを引く（登録式。実行ファイルが見つからなければ `None`）。
/// **transport（`lang::lsp`）は言語非依存**なので、ここに 1 行足すだけで言語が増える。
/// 診断/補完/定義の配線は LSP 標準なので変更不要（＝「拡張機構なしで N 言語」）。
fn language_server_for(path: &Path, remote: bool) -> Option<LanguageServer> {
    let extension = path.extension().and_then(|extension| extension.to_str())?.to_ascii_lowercase();
    let server = |language_id, command, args: &[&'static str]| LanguageServer {
        language_id,
        command,
        args: args.to_vec(),
    };
    let executable = |binary: &str| {
        if remote {
            Some(PathBuf::from(binary))
        } else {
            which(binary)
        }
    };
    match extension.as_str() {
        "rs" => {
            if remote {
                Some(server("rust", PathBuf::from("rust-analyzer"), &[]))
            } else {
                lsp_server_path().map(|command| server("rust", command, &[]))
            }
        }
        "ts" | "tsx" | "mts" | "cts" => {
            executable("typescript-language-server")
                .map(|command| server("typescript", command, &["--stdio"]))
        }
        "js" | "jsx" | "mjs" | "cjs" => {
            executable("typescript-language-server")
                .map(|command| server("javascript", command, &["--stdio"]))
        }
        "py" | "pyi" => executable("pyright-langserver")
            .map(|command| server("python", command, &["--stdio"]))
            .or_else(|| executable("pylsp").map(|command| server("python", command, &[]))),
        "go" => executable("gopls").map(|command| server("go", command, &[])),
        "c" | "h" => executable("clangd").map(|command| server("c", command, &[])),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => {
            executable("clangd").map(|command| server("cpp", command, &[]))
        }
        "lua" => executable("lua-language-server").map(|command| server("lua", command, &[])),
        "zig" => executable("zls").map(|command| server("zig", command, &[])),
        _ => None,
    }
}

/// PATH（＋ GUI 起動で欠けがちな共通 bin ディレクトリ）から実行ファイルを探す。
/// rust-analyzer と同じく、Finder 起動だと PATH が痩せるため候補ディレクトリを補う。
fn which(binary: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for suffix in [".local/bin", ".volta/bin", ".bun/bin", ".npm-global/bin", ".cargo/bin", ".deno/bin"] {
            dirs.push(home.join(suffix));
        }
    }
    for base in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        dirs.push(PathBuf::from(base));
    }
    dirs.into_iter().map(|dir| dir.join(binary)).find(|candidate| candidate.is_file())
}

/// rust-analyzer の実行パス。**rustup ツールチェーン内の実バイナリを優先**する
/// （`~/.cargo/bin/rust-analyzer` は rustup プロキシで、cwd/RUSTUP_TOOLCHAIN 依存に解決が変わり
/// GUI 起動時に "Unknown binary" で失敗するため避ける）。優先順: 環境変数 → toolchains/*/bin →
/// cargo プロキシ → PATH。
fn lsp_server_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("SHIRUSHI_RUST_ANALYZER") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(home) = &home {
        let toolchains = home.join(".rustup/toolchains");
        if let Ok(entries) = std::fs::read_dir(&toolchains) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("bin/rust-analyzer");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    if let Some(home) = &home {
        let proxy = home.join(".cargo/bin/rust-analyzer");
        if proxy.exists() {
            return Some(proxy);
        }
    }
    Some(PathBuf::from("rust-analyzer")) // PATH 上に任せる
}

/// パンくず文字列。ファイルがプロジェクト配下ならルート相対の各要素を ` › ` で連結。
/// 配下でなければファイル名のみ。
fn breadcrumb_text(root: Option<&Path>, path: Option<&Path>) -> String {
    let Some(path) = path else {
        return String::new();
    };
    let relative = match root.and_then(|root| path.strip_prefix(root).ok()) {
        Some(relative) => relative,
        None => {
            return path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
        }
    };
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" › ")
}

/// パスの末尾 `max` 階層のフォルダ名（ルート `/` は除く）。エクスプローラの上位階層ブレッドクラム用。
/// titlebar beacon のドット。実行中は breathing で pulse（停止中は淡色・静止）。
fn beacon_dot(id: impl Into<gpui::ElementId>, color: Hsla, running: bool) -> gpui::AnyElement {
    let base = if running { color } else { color.alpha(0.5) };
    let dot = div().size(px(8.)).rounded(px(4.)).flex_none().bg(base);
    if running {
        dot.with_animation(
            id,
            Animation::new(std::time::Duration::from_millis(1600))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.0)),
            |element, delta| element.opacity(delta),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

/// 拡張子別のアイコン色（識別用。theme の syntax パレットを流用＝色による方向感覚）。
fn file_type_color(name: &str, theme: &Theme) -> Hsla {
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let syntax = &theme.syntax;
    match extension.as_str() {
        "rs" => syntax.number,                                          // オレンジ
        "toml" | "yaml" | "yml" | "json" | "lock" | "ini" => syntax.function, // 青
        "md" | "markdown" | "txt" | "log" => theme.fg2,                 // ミュート
        "ts" | "tsx" | "js" | "mjs" | "cjs" | "jsx" => syntax.type_,    // 黄
        "py" | "rb" | "go" | "sh" | "zsh" | "bash" => syntax.string,    // 緑
        "html" | "htm" | "css" | "scss" | "vue" => syntax.keyword,      // 紫
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => syntax.macro_, // シアン
        _ => theme.fg1,
    }
}

/// エクスプローラのファイル/フォルダアイコン。フォルダ＝横長（folder 色）・ファイル＝縦長（型色）の
/// シルエットで一目で見分く。固定幅スロットに入れて名前を揃える。
fn file_icon(name: &str, is_dir: bool, theme: &Theme) -> impl IntoElement {
    let shape = if is_dir {
        let color = theme.folder_icon();
        div().w(px(13.)).h(px(10.)).rounded(px(2.5)).bg(color)
    } else {
        let color = file_type_color(name, theme);
        div().w(px(10.)).h(px(13.)).rounded(px(2.)).bg(color.alpha(0.9))
    };
    div().flex_none().w(px(16.)).flex().items_center().justify_center().child(shape)
}

/// アイコングリッド用の大きめアイコン（[`file_icon`] の 2 倍強）。
fn icon_large(name: &str, is_dir: bool, theme: &Theme) -> impl IntoElement {
    let shape = if is_dir {
        div().w(px(30.)).h(px(23.)).rounded(px(4.)).bg(theme.folder_icon())
    } else {
        let color = file_type_color(name, theme);
        div().w(px(22.)).h(px(28.)).rounded(px(3.5)).bg(color.alpha(0.9))
    };
    div().flex().items_center().justify_center().child(shape)
}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.editor {
            Some(editor) => editor.read(cx).focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_file_finder))
            .on_action(cx.listener(Self::open_project_switcher))
            .on_action(cx.listener(Self::open_project_search))
            .on_action(cx.listener(Self::open_theme_selector))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::go_to_definition))
            .on_action(cx.listener(Self::trigger_completion))
            .on_action(cx.listener(Self::toggle_split))
            .on_action(cx.listener(Self::toggle_git_panel))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::new_agent_thread))
            .on_action(cx.listener(Self::select_next_thread))
            .on_action(cx.listener(Self::select_prev_thread))
            .on_action(cx.listener(Self::new_window))
            // ⌘1..9 = レールのプロジェクト N 番へ切替（窓内切替・ウィンドウモデル §5）
            .on_action(cx.listener(|this, _: &ActivateProject1, window, cx| this.switch_project(0, window, cx)))
            .on_action(cx.listener(|this, _: &ActivateProject2, window, cx| this.switch_project(1, window, cx)))
            .on_action(cx.listener(|this, _: &ActivateProject3, window, cx| this.switch_project(2, window, cx)))
            .on_action(cx.listener(|this, _: &ActivateProject4, window, cx| this.switch_project(3, window, cx)))
            .on_action(cx.listener(|this, _: &ActivateProject5, window, cx| this.switch_project(4, window, cx)))
            .on_action(cx.listener(|this, _: &ActivateProject6, window, cx| this.switch_project(5, window, cx)))
            .on_action(cx.listener(|this, _: &ActivateProject7, window, cx| this.switch_project(6, window, cx)))
            .on_action(cx.listener(|this, _: &ActivateProject8, window, cx| this.switch_project(7, window, cx)))
            .on_action(cx.listener(|this, _: &ActivateProject9, window, cx| this.switch_project(8, window, cx)))
            .on_mouse_move(cx.listener(Self::on_resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_resize_end))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg0)
            .text_color(theme.fg0)
            .font_family("IBM Plex Sans JP") // UI = IBM Plex Sans JP（bin で bundle 済み）
            .text_size(px(12.5))
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .when(self.show_left, |element| {
                        // 左カラムは git パネル（ソース管理）とエクスプローラを ⌃⇧G で切替。
                        let column = if self.git_panel.is_some() {
                            self.render_git_panel(cx)
                        } else {
                            self.render_explorer(cx).into_any_element()
                        };
                        element.child(column)
                    })
                    .child(self.render_center(cx))
                    .when(self.show_right, |element| element.child(self.render_agent_dock(cx))),
            )
            .child(self.render_statusbar(cx))
            // オーバーレイ（最前面）
            .when_some(self.picker.clone(), |this, picker| this.child(picker))
            .children(self.render_search_panel(cx))
            .children(self.render_completion(cx))
            .children(self.render_branch_menu(cx))
            .children(self.render_explorer_context_menu(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_completion_items_handles_array_and_list() {
        // Array 形式 + textEdit/insertText/label の優先順。
        let array = json!([
            { "label": "push_str", "kind": 2, "detail": "fn", "textEdit": { "newText": "push_str(${0})" } },
            { "label": "len", "kind": 2, "insertText": "len()" },
            { "label": "Vec", "kind": 22 }
        ]);
        let items = parse_completion_items(&array);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].label.as_ref(), "push_str");
        assert_eq!(items[0].insert_text, "push_str(${0})"); // textEdit 優先
        assert_eq!(items[0].kind.as_ref(), "fn");
        assert_eq!(items[1].insert_text, "len()"); // insertText
        assert_eq!(items[2].insert_text, "Vec"); // label fallback
        assert_eq!(items[2].kind.as_ref(), "type"); // kind 22 = Struct

        // List 形式（items フィールド）。
        let list = json!({ "isIncomplete": false, "items": [ { "label": "x" } ] });
        assert_eq!(parse_completion_items(&list).len(), 1);
    }

    #[test]
    fn parse_definition_handles_location_shapes() {
        // 単一 Location
        let location = json!({ "uri": "file:///x/lib.rs", "range": { "start": { "line": 10, "character": 4 }, "end": { "line": 10, "character": 9 } } });
        assert_eq!(
            parse_definition(&location),
            Some((PathBuf::from("/x/lib.rs"), 10, 4))
        );
        // Location[]（先頭）
        let array = json!([location.clone()]);
        assert_eq!(parse_definition(&array), Some((PathBuf::from("/x/lib.rs"), 10, 4)));
        // LocationLink[]（targetUri + targetSelectionRange）
        let link = json!([{ "targetUri": "file:///y/m.rs", "targetSelectionRange": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 2 } } }]);
        assert_eq!(parse_definition(&link), Some((PathBuf::from("/y/m.rs"), 3, 0)));
        // null
        assert_eq!(parse_definition(&serde_json::Value::Null), None);
    }
}
