//! workspace — レール（プロジェクト切替）+ 左ドックのエクスプローラ + 中央エディタ + ステータスバー。
//!
//! ARCHITECTURE §5: 1 窓 = アクティブな (project, branch)。レール = 窓内切替。UI-SPEC §1.3 の色の
//! 許可リストに従い、**プロジェクト色**はレール枠/リング・ツリー選択の左バー・キャレットにのみ流す。
//! 状態（開プロジェクト・アクティブ・開ファイル）は `state.json` に保存し、再起動で復元する。

use agent_panel::AgentPanel;
use crate::persistence::{PersistedProject, PersistedState, RestoredTabs, state_path};
use crate::updater;
use editor_core::{Buffer, Selection};
use futures::StreamExt as _; // LSP 通知 pump の `.next()`
use editor_view::{ComposerEvent, EditorHoverEvent, EditorInputEvent, EditorView, PositionSnapshot};
use gpui::{
    Animation, AnimationExt, App, Bounds, ClipboardItem, Context, CursorStyle, Div, Entity,
    FocusHandle, Focusable, FontWeight, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Point, SharedString, Stateful, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowControlArea, WindowOptions, actions, div, point, prelude::*,
    pulsating_between, px, size, svg,
};
use host::Host;
use lang::lsp::{
    apply_text_edits_to_string, language_server_for, parse_definition, parse_hover_lines,
    parse_text_edits, parse_workspace_edit,
};
use project::{GitWorktree, GraphCommit, ProjectSource, StatusKind, WorkingChange, Worktree};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use terminal_view::{TerminalDock, TerminalDockEvent, TerminalLaunch};
use theme_core::{Theme, ThemeSource, project_color};
use ui::Tooltip;
use ui::{DraggedFile, Picker, PickerEvent, PickerItem};

actions!(
    workspace,
    [
        FileFinder,
        ProjectSwitcher,
        ProjectSearch,
        // ⌘F バッファ内検索 / ⌥⌘F 置換つき（M10）。
        BufferSearch,
        BufferReplace,
        ThemeSelector,
        // ⌘K⌘C プロジェクト色ピッカー（Peacock 拡張）。
        ProjectColor,
        ToggleTerminal,
        GoToDefinition,
        TriggerCompletion,
        // hover をキーで出す（マウス dwell と同じポップアップ。⌘K ⌘I = VSCode 互換）。
        ShowHover,
        // ナビゲーション履歴（⌃- 戻る / ⌃⇧- 進む・M10-11）。
        NavigateBack,
        NavigateForward,
        // ⌃G 行ジャンプ（M10-12）。
        GoToLine,
        // フォーマット（⌥⇧F・M11）と保存（⌘S = 保存時フォーマットのフック地点）。
        Format,
        SaveActive,
        // rename（F2・M11）。
        Rename,
        // ⌘I インライン編集（選択+指示 → その場 diff → accept/reject・M12-8）。
        InlineEdit,
        // Todo ボード（.shirushi/todos.md・M12-10）。
        ToggleTodoBoard,
        // ⌘⇧P コマンドパレット（M13）。
        CommandPalette,
        // リモート SSH ホストピッカー（~/.ssh/config・M13）。
        RemoteSsh,
        // スレッド履歴（過去スレッド一覧 → 復元・#5）。
        ThreadHistory,
        // code actions（⌘.・M11）と参照検索（⇧F12・M11）。
        CodeActions,
        FindReferences,
        // diff タブ（HEAD vs バッファ・M11-9）と hunk 移動（F7・M11-9）。
        OpenDiff,
        NextHunk,
        PrevHunk,
        // シンボル（⌘⇧O アウトライン / ⌘T ワークスペース・M11）と診断移動（F8・M11）。
        OutlineSymbols,
        WorkspaceSymbols,
        NextDiagnostic,
        PrevDiagnostic,
        DiagnosticsPanel,
        SplitRight,
        ToggleGitPanel,
        CloseTab,
        RestoreClosedTab,
        NewThread,
        // エディタタブの切替（⌘{ / ⌘} = ⌘⇧[ / ⌘⇧]）。M10 複数タブ。
        SelectNextTab,
        SelectPrevTab,
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
    /// ⌘⇧O アウトライン（tree-sitter）。id は `picker_symbol_rows` の添字。
    Symbols,
    /// ⌘T ワークスペースシンボル（LSP）。id は `picker_workspace_symbols` の添字。
    WorkspaceSymbols,
    /// ⌘⇧P コマンドパレット（M13）。id は [`command_entries`] の添字。
    Commands,
    /// リモート SSH ホストピッカー（M13）。id は `picker_ssh_hosts` の添字（末尾 id = 手入力）。
    SshHosts,
    /// スレッド履歴（過去スレッド一覧・#5）。id は `picker_history` の添字。
    ThreadHistory,
}

/// コマンドパレットの登録表（M13・CommandRegistry の最小形）: (i18n キー, アクション名)。
/// 新しい workspace アクションを足したらここにも 1 行足す（登録式 = パレットがコアを知らない）。
fn command_entries() -> &'static [(&'static str, &'static str)] {
    &[
        ("cmd.file_finder", "workspace::FileFinder"),
        ("cmd.save_active", "workspace::SaveActive"),
        ("cmd.close_tab", "workspace::CloseTab"),
        ("cmd.restore_closed_tab", "workspace::RestoreClosedTab"),
        ("cmd.project_switcher", "workspace::ProjectSwitcher"),
        ("cmd.new_window", "workspace::NewWindow"),
        ("cmd.buffer_search", "workspace::BufferSearch"),
        ("cmd.buffer_replace", "workspace::BufferReplace"),
        ("cmd.project_search", "workspace::ProjectSearch"),
        ("cmd.find_references", "workspace::FindReferences"),
        ("cmd.go_to_line", "workspace::GoToLine"),
        ("cmd.go_to_definition", "workspace::GoToDefinition"),
        ("cmd.navigate_back", "workspace::NavigateBack"),
        ("cmd.navigate_forward", "workspace::NavigateForward"),
        ("cmd.outline_symbols", "workspace::OutlineSymbols"),
        ("cmd.workspace_symbols", "workspace::WorkspaceSymbols"),
        ("cmd.next_diagnostic", "workspace::NextDiagnostic"),
        ("cmd.prev_diagnostic", "workspace::PrevDiagnostic"),
        ("cmd.diagnostics_panel", "workspace::DiagnosticsPanel"),
        ("cmd.format", "workspace::Format"),
        ("cmd.rename", "workspace::Rename"),
        ("cmd.code_actions", "workspace::CodeActions"),
        ("cmd.inline_edit", "workspace::InlineEdit"),
        ("cmd.trigger_completion", "workspace::TriggerCompletion"),
        ("cmd.show_hover", "workspace::ShowHover"),
        ("cmd.open_diff", "workspace::OpenDiff"),
        ("cmd.next_hunk", "workspace::NextHunk"),
        ("cmd.prev_hunk", "workspace::PrevHunk"),
        ("cmd.theme_selector", "workspace::ThemeSelector"),
        ("cmd.project_color", "workspace::ProjectColor"),
        ("cmd.toggle_terminal", "workspace::ToggleTerminal"),
        ("cmd.toggle_git_panel", "workspace::ToggleGitPanel"),
        ("cmd.toggle_todo_board", "workspace::ToggleTodoBoard"),
        ("cmd.split_right", "workspace::SplitRight"),
        ("cmd.new_thread", "workspace::NewThread"),
        ("cmd.next_tab", "workspace::SelectNextTab"),
        ("cmd.prev_tab", "workspace::SelectPrevTab"),
        ("cmd.next_thread", "workspace::SelectNextThread"),
        ("cmd.prev_thread", "workspace::SelectPrevThread"),
        ("cmd.remote_ssh", "workspace::RemoteSsh"),
        ("cmd.thread_history", "workspace::ThreadHistory"),
    ]
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
/// macOS のネイティブ信号機（appears_transparent 時も残る）を避けるための左余白。
const TRAFFIC_LIGHT_INSET: f32 = 92.0; // ネイティブ信号機とプロジェクトピルの間に余白を持たせる

/// titlebar / statusbar のドックトグルが指すドック。
#[derive(Clone, Copy, PartialEq)]
enum Dock {
    Left,
    Right,
    Bottom,
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

/// ツリーのインライン命名（新規ファイル/フォルダ・リネーム・M10）。
struct ExplorerNaming {
    kind: NamingKind,
    /// 作成先の親フォルダ（rename では対象の親）。
    parent: PathBuf,
    /// rename の対象（New* では None）。
    target: Option<PathBuf>,
    value: String,
    focus: FocusHandle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NamingKind {
    NewFile,
    NewDir,
    Rename,
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
    /// このプロジェクトで開いているタブのファイル一覧（左から順・M10 複数タブ）。
    /// アクティブプロジェクトでは `Workspace.tabs` が真実で、ここへは同期する（`sync_active_slot`）。
    /// 非アクティブプロジェクトでは復元用の記録（切替時に開き直す）。
    open_files: Vec<PathBuf>,
    /// アクティブタブの添字（`open_files` 内）。
    active_file: usize,
    /// カラム/アイコン表示の「現在フォルダ」（ここの直下を見せる）。既定はルート。
    current_dir: Option<PathBuf>,
    /// `.shirushi/settings.json` の絵文字アイコン（M12-11。None = 頭文字モノグラム）。
    icon: Option<SharedString>,
    /// リンク worktree として開いたスロットのブランチ名（Some = worktree タブ・M10-2）。
    /// レール右クリックの「worktree を削除 / ブランチを削除」を出す判定に使う。通常/メインは None。
    worktree_branch: Option<String>,
    /// アイコン/カラム表示のディレクトリ列挙キャッシュ。**render 中の FS/RPC を初回 1 回に抑える**
    /// （ARCHITECTURE §9）。watch の refresh / プロジェクト再読込で無効化。RefCell は render(&self) から埋めるため。
    dir_listings: std::cell::RefCell<HashMap<PathBuf, Vec<project::Entry>>>,
}

/// ペインに載る 1 タブ（M10 複数タブ）。当面は具体型（エディタ）だけ・多態化は必要時に育てる
/// （ARCHITECTURE §3 の Pane/Item 初版）。`path` はタブの同一判定と永続化のキー。
struct EditorTab {
    path: PathBuf,
    editor: Entity<EditorView>,
    /// 一時タブ（diff タブ等・M11-9）。永続化・⌘⇧T 復元・LSP から除外する。
    transient: bool,
    /// このタブのバッファ変更監視（再描画 + LSP didChange）。タブごとに持つ。
    _observation: Subscription,
    /// 確定入力の購読（補完の自動トリガ・M10）。
    _input_subscription: Subscription,
    /// hover dwell の購読（LSP hover・M10）。
    _hover_subscription: Subscription,
}

/// ドラッグ中のエディタタブ（Chrome 風並べ替えのゴースト。`agent_panel` の DraggedThreadTab と同型）。
#[derive(Clone)]
struct DraggedEditorTab {
    index: usize,
    name: SharedString,
    theme: Theme,
}

impl Render for DraggedEditorTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(self.theme.bg2)
            .border_1()
            .border_color(self.theme.border)
            .text_size(px(12.))
            .text_color(self.theme.fg0)
            .child(self.name.clone())
    }
}

struct BranchMenuState {
    position: Point<gpui::Pixels>,
    current: Option<String>,
    branches: Vec<String>,
    worktrees: Vec<GitWorktree>,
}

/// レール項目の右クリックメニュー（色スウォッチ + 新規窓 / レールから外す / worktree・ブランチ削除・M10-2）。
struct RailMenuState {
    /// 対象プロジェクトのレール index。
    project_index: usize,
    /// ポップオーバー表示位置（右クリック位置）。
    position: Point<gpui::Pixels>,
    /// 破壊的操作の二段確認（Some = この操作が確認待ち。別項目クリックや再オープンで解除）。
    confirm: Option<RailMenuAction>,
}

/// レールメニューの破壊的操作（二段確認の対象）。
#[derive(Clone, Copy, PartialEq)]
enum RailMenuAction {
    /// worktree を削除（git worktree remove）。スロットも外す。
    RemoveWorktree,
    /// worktree ごとブランチを削除（worktree remove → git branch -D）。スロットも外す。
    DeleteBranch,
}

impl ProjectSlot {
    fn refresh(&mut self) {
        let mut rows = Vec::new();
        let root = self.worktree.root().to_path_buf();
        build_rows(&self.worktree, &root, 0, &self.expanded, &mut rows);
        self.rows = rows;
        self.dir_listings.borrow_mut().clear(); // FS が変わった合図 → アイコン/カラムの列挙も取り直す
    }

    /// アイコン/カラム表示用のディレクトリ列挙（キャッシュ付き）。初回だけ FS/RPC を読み、
    /// 以後の render はキャッシュを返す（無効化は [`Self::refresh`]）。
    fn listed_dir(&self, dir: &Path) -> Vec<project::Entry> {
        if let Some(cached) = self.dir_listings.borrow().get(dir) {
            return cached.clone();
        }
        let entries = self.worktree.read_any_dir(dir).unwrap_or_default();
        self.dir_listings.borrow_mut().insert(dir.to_path_buf(), entries.clone());
        entries
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

/// ⌘F バッファ内検索の表示上限。これを超えるマッチは n/m に「+」を付けて数えない
/// （全置換は打ち切らず全件を対象にする）。
const BUFFER_SEARCH_MAX: usize = 20_000;

/// ⌘F バッファ内検索/置換バーの状態（アクティブタブのエディタに対して働く・M10）。
/// マッチ列はエディタ毎の状態なので、タブ/プロジェクト切替では畳む（持ち越さない）。
struct BufferSearchState {
    query: String,
    replace: String,
    case_sensitive: bool,
    is_regex: bool,
    /// 置換行を表示しているか（⌥⌘F / ▸クリックで開く）。
    show_replace: bool,
    /// タイプ先が置換フィールドか（Tab / フィールドクリックで切替）。
    editing_replace: bool,
    /// 現在のマッチ列（byte レンジ・start 昇順・最大 [`BUFFER_SEARCH_MAX`] 件）。
    matches: Vec<std::ops::Range<usize>>,
    /// 上限で打ち切られたか（n/m に「+」を付ける）。
    truncated: bool,
    /// 現在マッチ（`matches` 内の添字）。
    current: usize,
    /// 直近クエリのコンパイルエラー（正規表現の誤り等）。
    error: Option<SharedString>,
    /// この (バッファ version, クエリ, 大小, 正規表現) で計算済み。
    /// エディタの observe は blink 等でも発火するため、これで再計算を止める。
    computed_for: Option<(u64, String, bool, bool)>,
    /// ⌘F を開いた時点の位置（Esc で戻す）。
    saved_position: PositionSnapshot,
    focus: FocusHandle,
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
/// 印字キーは type-through（エディタへ挿入 → prefix で**クライアント側絞り込み**・LSP 再要求なし）。
struct CompletionState {
    /// LSP が返した全候補（絞り込み前）。
    items: Vec<CompletionItem>,
    /// 現在の絞り込みプレフィクス（キャレット前の識別子。タイプで伸びる）。
    prefix: String,
    /// 絞り込み後リスト内の選択位置。
    selected: usize,
    position: Point<gpui::Pixels>,
    focus: FocusHandle,
}

impl CompletionState {
    /// prefix で絞り込んだ候補の添字列（`items` へのインデックス）。
    fn filtered(&self) -> Vec<usize> {
        filter_completion_indices(&self.items, &self.prefix)
    }
}

/// 候補を prefix（大小無視の前方一致）で絞り込み、残った候補の添字列を返す。
fn filter_completion_indices(items: &[CompletionItem], prefix: &str) -> Vec<usize> {
    if prefix.is_empty() {
        return (0..items.len()).collect();
    }
    let needle = prefix.to_lowercase();
    (0..items.len())
        .filter(|&index| items[index].label.to_lowercase().starts_with(&needle))
        .collect()
}

/// タイプされたテキストが補完の自動トリガに当たるか（M10）。
/// `before` はキャレット直前のテキスト（挿入後・2 バイトあれば十分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionTrigger {
    /// 識別子文字（絞り込み継続 or 新語でポップアップ）。
    Identifier,
    /// `.` / `::`（メンバ/パス補完を新規要求）。
    Fresh,
    /// トリガしない（ポップアップが開いていれば閉じる）。
    None,
}

/// ⌘I インライン編集（M12-8）。選択+指示 → `claude -p` で書き換え → その場 diff → accept/reject。
/// チャット（スレッド）へ行かない最短経路。ai_commit_message と同じ「CLI に流す」型。
struct InlineEditState {
    /// 指示の入力値。
    instruction: String,
    focus: FocusHandle,
    /// 何を書き換えるか（エディタの範囲 / ターミナルのコマンド生成）。
    target: InlineEditTarget,
    /// 生成中（`claude -p` 実行中）。
    busy: bool,
    /// 生成結果（Some = プレビュー + accept/reject 表示）。
    proposal: Option<String>,
    /// 世代（Esc 後に届いた古い結果を捨てる）。
    generation: u64,
    /// 失敗メッセージ（claude 未導入など）。
    error: Option<String>,
}

/// ⌘I の対象（M12-8）。エディタとターミナルで同型の体験にする。
enum InlineEditTarget {
    /// エディタの選択範囲を書き換える。
    Editor {
        /// 対象のバイト範囲（開いた時点の選択。適用まで不変とみなし、version で守る）。
        range: Range<usize>,
        /// 対象の旧テキスト。
        old_text: String,
        /// 開いた時点のバッファ version（適用時に不一致なら破棄 = 安全側）。
        buffer_version: u64,
    },
    /// 自然言語 → シェルコマンド 1 行を生成し、ターミナル入力行へ挿入（実行はユーザーの Enter）。
    Terminal,
}

/// ⌘I の diff プレビュー行（unified diff からファイルヘッダを除き、長い時は中央を省略）。
/// ハンク境界 `@@` は「···」に置き換える（行番号は「その場」プレビューでは意味が薄い）。
fn inline_edit_diff_lines(old_text: &str, new_text: &str, max_lines: usize) -> Vec<String> {
    let Some(diff) = project::unified_diff_texts(old_text, new_text, "inline") else {
        return vec![i18n::t!("inline.no_change").to_string()];
    };
    let lines: Vec<String> = diff
        .lines()
        .filter(|line| !(line.starts_with("---") || line.starts_with("+++")))
        .map(|line| if line.starts_with("@@") { "···".to_string() } else { line.to_string() })
        .collect();
    if lines.is_empty() {
        return vec![i18n::t!("inline.no_change").to_string()];
    }
    if lines.len() <= max_lines {
        return lines;
    }
    let head = max_lines / 2;
    let tail = max_lines - head - 1;
    let mut out: Vec<String> = lines[..head].to_vec();
    out.push(format!("··· (+{})", lines.len() - head - tail));
    out.extend_from_slice(&lines[lines.len() - tail..]);
    out
}

/// 自動アップデートの段階（M13）。statusbar チップの表示を兼ねる。
#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateState {
    /// 新版あり（クリックで更新開始）。
    Available,
    /// ダウンロード + 検証 + 差し替え中。
    Installing,
    /// 差し替え済み（再起動で反映）。
    Ready,
}

/// Todo ボード（M12-10）。真実は `.shirushi/todos.md` — UI はただの書き手のひとつ。
struct TodoBoardState {
    items: Vec<project::todos::TodoItem>,
    /// ✨今日の計画（`claude -p`）の実行中。
    plan_busy: bool,
    /// ▶ で AI に送った実行中項目（行番号 → スレッド色）。TurnEnded で解除。
    running: HashMap<usize, Hsla>,
    /// ＋ で追加中のタスク入力（IME 正しい EditorView::plain）。None = 追加してない。
    add_input: Option<Entity<EditorView>>,
}

/// ⌘. code actions のポップアップ（M11）。補完と同型（フォーカスを取り上下/Enter/Esc）。
struct CodeActionsState {
    /// (タイトル, アクションの生 JSON)。
    items: Vec<(SharedString, serde_json::Value)>,
    selected: usize,
    position: Point<gpui::Pixels>,
    focus: FocusHandle,
}

/// hover ポップアップ（LSP `textDocument/hover` の結果・M10）。フォーカスは取らない。
struct HoverState {
    /// プレーン表示する行（markdown はコードフェンス行だけ落とした素通し）。
    lines: Vec<SharedString>,
    /// 表示アンカー（dwell したマウス位置 / キャレット位置）。
    position: Point<gpui::Pixels>,
}

fn classify_completion_trigger(typed: &str, before: &str) -> CompletionTrigger {
    let Some(last) = typed.chars().last() else {
        return CompletionTrigger::None;
    };
    if last.is_ascii_alphanumeric() || last == '_' {
        return CompletionTrigger::Identifier;
    }
    if last == '.' {
        return CompletionTrigger::Fresh;
    }
    // `::` = 直前 2 文字が "::"（1 個目の `:` では出さない）。
    if last == ':' && before.ends_with("::") {
        return CompletionTrigger::Fresh;
    }
    CompletionTrigger::None
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

/// LSP Location[] を検索パネルの [`search::FileMatch`] 群へ（プレビュー行はファイルを読んで作る）。
fn locations_to_file_matches(host: &dyn Host, value: &serde_json::Value) -> Vec<search::FileMatch> {
    let Some(array) = value.as_array() else {
        return Vec::new();
    };
    // path → (line, start_char, end_char)
    let mut by_file: Vec<(PathBuf, Vec<(u32, u32, u32)>)> = Vec::new();
    for location in array {
        let Some(uri) = location.get("uri").and_then(|uri| uri.as_str()) else {
            continue;
        };
        let Some(path) = lang::lsp::uri_to_path(uri) else {
            continue;
        };
        let (Some(line), Some(start_char), Some(end_char)) = (
            location.pointer("/range/start/line").and_then(|v| v.as_u64()),
            location.pointer("/range/start/character").and_then(|v| v.as_u64()),
            location.pointer("/range/end/character").and_then(|v| v.as_u64()),
        ) else {
            continue;
        };
        match by_file.iter_mut().find(|(existing, _)| existing == &path) {
            Some((_, hits)) => hits.push((line as u32, start_char as u32, end_char as u32)),
            None => by_file.push((path, vec![(line as u32, start_char as u32, end_char as u32)])),
        }
    }
    let mut results = Vec::new();
    for (path, mut hits) in by_file {
        hits.sort_unstable();
        let lines: Vec<String> = host
            .read_file(&path)
            .ok()
            .and_then(|content| String::from_utf8(content.bytes).ok())
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default();
        let matches = hits
            .into_iter()
            .map(|(line, start_char, end_char)| {
                let line_text = lines.get(line as usize).cloned().unwrap_or_default();
                // UTF-16 char → 行内 byte。
                let to_byte = |character: u32| -> usize {
                    let mut utf16 = 0usize;
                    for (offset, c) in line_text.char_indices() {
                        if utf16 >= character as usize {
                            return offset;
                        }
                        utf16 += c.len_utf16();
                    }
                    line_text.len()
                };
                let column = to_byte(start_char);
                let end = to_byte(end_char).max(column);
                search::Match {
                    line: line as usize,
                    column,
                    byte_range: 0..(end - column), // 長さだけ使われる（強調幅）
                    line_text,
                }
            })
            .collect();
        results.push(search::FileMatch { path, matches });
    }
    results
}

/// `.shirushi/settings.json` の `color`（"#rrggbb"）と `icon`（絵文字）を読む（M12-11）。
/// 無ければ (None, None)。パース失敗は無視して既定へ。
fn read_project_identity(root: &Path) -> (Option<Hsla>, Option<SharedString>) {
    let path = root.join(".shirushi/settings.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (None, None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (None, None);
    };
    let color = value
        .get("color")
        .and_then(|color| color.as_str())
        .and_then(parse_hex_color);
    let icon = value
        .get("icon")
        .and_then(|icon| icon.as_str())
        .filter(|icon| !icon.is_empty())
        .map(|icon| SharedString::from(icon.to_string()));
    (color, icon)
}

/// "#rrggbb" → Hsla。
fn parse_hex_color(hex: &str) -> Option<Hsla> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    let r = ((value >> 16) & 0xff) as f32 / 255.0;
    let g = ((value >> 8) & 0xff) as f32 / 255.0;
    let b = (value & 0xff) as f32 / 255.0;
    // RGB → HSL（gpui は hsla）。
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    let saturation = if delta == 0.0 {
        0.0
    } else {
        delta / (1.0 - (2.0 * lightness - 1.0).abs())
    };
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    Some(gpui::hsla(hue, saturation, lightness, 1.0))
}

/// 2 色がほぼ同じか（HSLA 各成分を小さな誤差で比較）。レールの色衝突判定に使う。
fn colors_close(a: Hsla, b: Hsla) -> bool {
    const EPSILON: f32 = 0.01;
    (a.h - b.h).abs() < EPSILON
        && (a.s - b.s).abs() < EPSILON
        && (a.l - b.l).abs() < EPSILON
        && (a.a - b.a).abs() < EPSILON
}

/// スロットを `removed` でレールから外した後の active index（`new_len` = 外した後の projects 数・>=1 前提）。
/// active より前を消せば 1 つ前へ・後ろを消せば不変・active 自身を消せば同 index（末尾なら 1 つ前）。
fn active_index_after_removal(active: usize, removed: usize, new_len: usize) -> usize {
    if active > removed {
        active - 1
    } else if active == removed {
        active.min(new_len - 1)
    } else {
        active
    }
}

// ── Workspace 本体 ──

pub struct Workspace {
    projects: Vec<ProjectSlot>,
    active: usize,
    // 主ペインの開いているタブ（左から順）。アクティブプロジェクトの分だけを持つ。M10 複数タブ。
    tabs: Vec<EditorTab>,
    // アクティブタブの添字（`tabs` 内）。tabs が空なら意味を持たない。
    active_tab: usize,
    // 右分割ペイン（⌘\ で開閉。独立エディタ＝比較・参照用の副ビュー。LSP/統合は主ペイン=tabs 側）。
    split_editor: Option<Entity<EditorView>>,
    agent_panel: Entity<AgentPanel>,
    theme: Theme,
    focus_handle: FocusHandle,
    state_path: Option<PathBuf>,
    // ドックの表示状態（titlebar のアイコンでトグル）。右=Agent パネル・下=ターミナルは M4/M8。
    show_left: bool,
    show_right: bool,
    show_bottom: bool,
    // 設定ホーム（中央領域・レール ⚙ で開く。第1セクション=Agents セットアップ・M12）。
    show_settings: bool,
    // オンボーディング完了の祝い紙吹雪（true の間だけルートに降らせる。~2.2s で自動オフ）。
    confetti: bool,
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
    // ツリーのインライン命名（新規/リネーム・M10）。Some の間は入力行を描く。
    explorer_naming: Option<ExplorerNaming>,
    // プロジェクト横断検索パネル（開いていれば Some・⌘⇧F）。
    search_panel: Option<SearchState>,
    next_search_id: u64,
    // ⌘F バッファ内検索/置換バー（開いていれば Some。アクティブタブのエディタが対象）。
    buffer_search: Option<BufferSearchState>,
    // アクティブプロジェクトの git 状態（絶対パス → 状態）。ツリー/タブの色分けに使う。
    git_status: HashMap<PathBuf, StatusKind>,
    // Git パネルを描画するたびに process/RPC を起動しないための snapshot。
    git_changes: Vec<WorkingChange>,
    git_history: Vec<GraphCommit>,
    // origin が GitHub なら owner/repo（PR ボタンの表示判定。M8: GitHub 連携）。
    github_slug: Option<String>,
    // branch/worktree メニュー（titlebar の ⎇ クリックで開く。開いていれば出す位置）。
    branch_menu: Option<BranchMenuState>,
    // 下ドックの統合ターミナル。タブ・PTY の所有権は terminal_view 側に閉じる。
    terminal_dock: Entity<TerminalDock>,
    // 最後に触った面が Agent か（⌘W の宛先判定。true=Agent スレッド / false=エディタタブ）。
    // フォーカスに頼らずクリックで確定する（Zed の active pane 相当）。
    agent_active: bool,
    // ⌘W で閉じたファイルの履歴（⌘⇧T で復元。Chrome 風）。新しいものが末尾。
    recently_closed_files: Vec<PathBuf>,
    // ── LSP（言語サーバ・M7。拡張子→サーバの登録式で多言語対応）──
    lsp: Option<lang::lsp::LspClient>,
    lsp_root: Option<PathBuf>,
    // 現在稼働中のサーバが担当する languageId（"rust"/"typescript"/… ）。ファイル言語が
    // 変わったら張り替える判定に使う。
    lsp_language: Option<&'static str>,
    lsp_initialized: bool,
    // サーバが Incremental didChange を広告したか（initialize 応答の textDocumentSync・M11-8）。
    lsp_incremental_sync: bool,
    // ファイル別に最後に didChange で送ったバッファ version（重複送信の抑止）。
    // 複数タブでは version 番号がファイル間で衝突しうるので path 別に持つ（単一 u64 だと誤スキップ）。
    lsp_sent_versions: HashMap<PathBuf, u64>,
    // ファイル別診断（絶対パス → (行, 重大度)）。gutter/statusbar に使う。
    diagnostics: HashMap<PathBuf, Vec<(u32, lang::lsp::Severity)>>,
    // 生の LSP Diagnostic[]（⌘. の codeAction context に渡す・M11）。
    raw_diagnostics: HashMap<PathBuf, serde_json::Value>,
    _lsp_pump: Option<gpui::Task<()>>,
    // 補完ポップアップ（開いていれば。Ctrl-Space / 自動トリガで開く）。
    completion: Option<CompletionState>,
    // 補完要求の世代番号（連打時に古い応答でポップアップを出さない）。
    completion_generation: u64,
    // Esc で閉じた語の語頭 offset（同じ語の入力継続では自動再表示しない）。
    completion_suppressed_word: Option<usize>,
    // hover ポップアップ（マウス dwell / ⌘K ⌘I で開く。フォーカスは取らない）。
    hover: Option<HoverState>,
    // hover 要求の世代番号（古い応答・マウス移動後の応答を捨てる）。
    hover_generation: u64,
    // git 状態リフレッシュの世代（背景実行の古い結果を捨てる）。
    git_refresh_gen: u32,
    // ⌃G 行ジャンプの入力（Some = ミニオーバーレイ表示中）。
    goto_line: Option<(String, FocusHandle)>,
    // ⌘⇧O アウトラインの行番号（Picker id → 行）。
    picker_symbol_rows: Vec<usize>,
    // ⌘T ワークスペースシンボルの着地先（Picker id → (パス, 行, 文字)）。
    picker_workspace_symbols: Vec<(PathBuf, u32, u32)>,
    // ⌘O の worktree 行（Picker id-1000 → worktree パス・M12-12）。
    picker_worktree_rows: Vec<PathBuf>,
    /// SSH ホストピッカー（M13）の裏付け。id → config ホスト（範囲外 id = 末尾「手入力」）。
    picker_ssh_hosts: Vec<host::SshConfigHost>,
    /// SSH ピッカーの「最近のリモートプロジェクト」行の接続 uri（id 前半・#5）。
    picker_ssh_recent: Vec<String>,
    /// スレッド履歴 Picker の (id, name, color_index)。id は表示添字（#5）。
    picker_history: Vec<(String, String, i64)>,
    // F2 rename の入力（Some = ミニオーバーレイ表示中。値は新しい名前）。
    rename_input: Option<(String, FocusHandle)>,
    // ⌘I インライン編集（Some = オーバーレイ表示中・M12-8）。
    inline_edit: Option<InlineEditState>,
    // Todo ボード（Some = 左カラムが板・M12-10。真実は .shirushi/todos.md）。
    todo_board: Option<TodoBoardState>,
    // ⌘. code actions のポップアップ。
    code_actions: Option<CodeActionsState>,
    // hunk 操作ポップオーバー（gutter の diff バークリック・M11-10）。
    hunk_menu: Option<(project::DiffHunk, Point<gpui::Pixels>)>,
    // レール右クリック / ⌘K⌘C の色ピッカー（プロジェクト index + 表示位置 + 任意 hex 入力・M12-11 / Peacock 拡張）。
    color_picker: Option<ColorPickerState>,
    // レール項目の右クリックメニュー（色 + 新規窓 / 外す / worktree・ブランチ削除・M10-2）。
    rail_menu: Option<RailMenuState>,
    // 承認カード「エディタで開く」の保留タブ（subscribe に window が無いため次の描画で開く・M12-6）。
    pending_transient_tab: Option<(PathBuf, Buffer)>,
    /// スレッド履歴を開く要求（Agent パネルのボタン → window が要るので次の render で消化・#5）。
    pending_open_history: bool,
    // ターミナル file:line リンクの保留ジャンプ（同上の window 制約・M13）。(パス, 0-based 行)。
    pending_terminal_jump: Option<(PathBuf, u32)>,
    // 自動アップデート（M13）。Some = statusbar にチップを出す。
    update_status: Option<(updater::UpdateInfo, UpdateState)>,
    // SSH 接続の入力バー（titlebar の SSH ボタン・M13）。Some = オーバーレイ表示中。
    ssh_input: Option<(String, FocusHandle)>,
    // SSH 接続の実行中（多重接続防止 + ボタンの busy 表示）。
    ssh_connecting: bool,
    // レール＋のフォルダ選択ダイアログ表示中（＋連打で Finder を多重に開かないためのガード・ssh_connecting と同型）。
    add_project_dialog_open: bool,
    // ＋で追加したプロジェクトへの切替待ち（ダイアログ経由は window が無い → render で消化）。
    pending_project_switch: Option<usize>,
    // トースト（右下・5 秒で消える・M12-5）。(本文, 色, 世代)。
    toasts: Vec<(SharedString, Hsla, u32)>,
    toast_gen: u32,
    // エージェントが触ったファイル → スレッド色（色リンク・M12-4。ツリー/タブのドットと gutter 色）。
    agent_touched: HashMap<PathBuf, Hsla>,
    // 権限待ちスレッドの色（statusbar ドット・M12-5。TurnEnded で消える）。
    waiting_thread: Option<(SharedString, Hsla)>,
    // blame の世代（デバウンス・古い結果破棄）と直近対象（キャレット行が変わった時だけ再計算・M11-11）。
    blame_gen: u32,
    last_blame_target: Option<(PathBuf, usize)>,
    // ナビゲーション履歴（ジャンプ級の移動の戻る/進む・M10-11）。
    nav_back: Vec<(PathBuf, usize)>,
    nav_forward: Vec<(PathBuf, usize)>,
    // ローカル永続化 DB（hot exit。開けなければ None = 機能は静かに無効）。
    storage: Option<storage::Storage>,
    // hot exit スナップショットのデバウンス世代。
    hot_exit_gen: u32,
    // hot exit を予約済みのバッファ version（blink の notify で再予約しない — LSP didChange と同型）。
    hot_exit_versions: HashMap<PathBuf, u64>,
    // 起動時に見つかった前回の未保存スナップショット（Some = 復元/破棄バーを出す）。
    hot_exit_pending: Option<Vec<(PathBuf, String)>>,
    // ファイル監視（アクティブプロジェクトの worktree・M10）。プロジェクト切替で張り直す。
    _watch: Option<project::Watch>,
    _watch_pump: Option<gpui::Task<()>>,
    // ── git 操作パネル（M8: ソース管理。⌃⇧G で左カラムをエクスプローラと切替）──
    git_panel: Option<GitPanelState>,
    // push/pull はネットワークで遅い → 背景実行中は true（ボタン無効化・表示用）。
    git_busy: bool,
}

/// プロジェクト色ピッカーの状態（識別用の厳選スウォッチ + 任意 hex 入力）。
/// hex 入力は rename と同じ流儀（keystroke を直接 String に積む）。
struct ColorPickerState {
    /// 対象プロジェクトのレール index。
    project_index: usize,
    /// ポップオーバー表示位置（右クリック位置 or アクティブ項目のアンカー）。
    position: Point<gpui::Pixels>,
    /// 任意 hex 入力の編集中文字列（`#` 抜き・0-9a-fA-F 最大 6 桁）。
    hex: String,
    /// キーストローク取り込み用フォーカス。
    focus: FocusHandle,
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
    /// リモートプロジェクトの窓色をローカル DB（storage）から解決する（M13 #3b）。
    /// `.shirushi` はリモート側にあり identity に使えないため、ホスト識別子 → 色をローカルに持つ。
    /// 初回は識別子から安定に 1 色を焼き付け、以後は同じ色（開き直し・レール並び順が変わっても不変）。
    fn apply_remote_host_colors(&mut self) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        for slot in &mut self.projects {
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
                        expanded: HashSet::new(),
                        rows: Vec::new(),
                        selected: None,
                        open_files: Vec::new(),
                        active_file: 0,
                        current_dir: None,
                        icon: identity.1,
                        worktree_branch: None,
                        dir_listings: std::cell::RefCell::new(HashMap::new()),
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
        let active = active.min(projects.len().saturating_sub(1));
        let agent_panel = cx.new(|cx| AgentPanel::new(theme.clone(), cx));
        let terminal_launch = Self::terminal_launch_for(projects.get(active));
        let terminal_dock = cx.new(|_| TerminalDock::new(terminal_launch, theme.clone()));
        cx.subscribe(&terminal_dock, Self::on_terminal_dock_event).detach();
        let mut workspace = Workspace {
            projects,
            active,
            tabs: Vec::new(),
            active_tab: 0,
            split_editor: None,
            agent_panel,
            theme,
            focus_handle: cx.focus_handle(),
            state_path,
            show_left: true,
            show_right: true, // Agent パネル（差別化の本丸）は既定で表示
            show_bottom: false,
            // 初回（未オンボード）は設定ホームを自動で開く。SHIRUSHI_SETTINGS=1 でも開く（撮影用）。
            show_settings: std::env::var_os("SHIRUSHI_SETTINGS").is_some()
                || !settings::get(cx).onboarded,
            // オンボーディング完了の祝い（紙吹雪）。SHIRUSHI_CONFETTI=1 で撮影用に出す。
            confetti: std::env::var_os("SHIRUSHI_CONFETTI").is_some(),
            agent_width: AGENT_DOCK_WIDTH,
            resizing_agent: false,
            resize_start_x: 0.0,
            resize_start_width: 0.0,
            explorer_width: DOCK_WIDTH,
            resizing_explorer: false,
            should_move_window: false,
            picker: None,
            picker_mode: PickerMode::Files,
            picker_files: Vec::new(),
            picker_themes: Vec::new(),
            theme_before_preview: None,
            _picker_observation: None,
            explorer_naming: None,
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
            buffer_search: None,
            git_status: HashMap::new(),
            git_changes: Vec::new(),
            git_history: Vec::new(),
            github_slug: None,
            branch_menu: None,
            terminal_dock,
            agent_active: false,
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
            git_refresh_gen: 0,
            goto_line: None,
            picker_symbol_rows: Vec::new(),
            picker_ssh_hosts: Vec::new(),
            picker_ssh_recent: Vec::new(),
            picker_history: Vec::new(),
            picker_workspace_symbols: Vec::new(),
            picker_worktree_rows: Vec::new(),
            rename_input: None,
            inline_edit: None,
            todo_board: None,
            code_actions: None,
            hunk_menu: None,
            color_picker: None,
            rail_menu: None,
            pending_transient_tab: None,
            pending_open_history: false,
            pending_terminal_jump: None,
            update_status: None,
            ssh_input: None,
            ssh_connecting: false,
            add_project_dialog_open: false,
            pending_project_switch: None,
            toasts: Vec::new(),
            toast_gen: 0,
            agent_touched: HashMap::new(),
            waiting_thread: None,
            blame_gen: 0,
            last_blame_target: None,
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            storage: None,
            hot_exit_gen: 0,
            hot_exit_versions: HashMap::new(),
            hot_exit_pending: None,
            _watch: None,
            _watch_pump: None,
            git_panel: None,
            git_busy: false,
        };
        workspace.refresh_git_status(cx); // ツリー/タブの git 色分け用
        // 開発用: SHIRUSHI_GIT_PANEL=1 で git 操作パネル（ソース管理）を開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_GIT_PANEL").is_some() {
            workspace.git_panel =
                Some(GitPanelState { message: String::new(), branch_name: None, focus: cx.focus_handle() });
            workspace.refresh_git_status(cx);
        }
        // 開発用: SHIRUSHI_BRANCH_MENU=1 で branch/worktree メニューを開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_BRANCH_MENU").is_some() {
            workspace.toggle_branch_menu(point(px(90.), px(44.)), cx);
        }
        // 開発用: SHIRUSHI_TERMINAL=1 で下ドックのターミナルを開いた状態で撮る。
        if std::env::var_os("SHIRUSHI_TERMINAL").is_some() {
            workspace.show_bottom = true;
            workspace.terminal_dock.update(cx, |dock, cx| {
                dock.ensure_active(cx);
            });
        }
        // 開発用: SHIRUSHI_COLOR_PICKER=1（or hex 文字列）で色ピッカーを開いた状態で撮る（Peacock 拡張の検証）。
        if let Ok(probe) = std::env::var("SHIRUSHI_COLOR_PICKER") {
            let hex = if probe == "1" { String::new() } else { probe.trim_start_matches('#').to_string() };
            workspace.color_picker = Some(ColorPickerState {
                project_index: workspace.active,
                position: point(px(RAIL_WIDTH), px(12.)),
                hex,
                focus: cx.focus_handle(),
            });
        }
        // 開発用: SHIRUSHI_RAIL_MENU でレール項目の右クリックメニューを開いた状態で撮る（M10-2）。
        // 値: 1=通常 / confirm-worktree / confirm-branch=破壊的操作の二段確認 armed 状態。
        // worktree/ブランチ削除行も見せるため、アクティブスロットを worktree タブ扱いにする。
        if let Ok(mode) = std::env::var("SHIRUSHI_RAIL_MENU") {
            if let Some(slot) = workspace.projects.get_mut(workspace.active) {
                slot.worktree_branch = Some("feature/login".to_string());
            }
            let confirm = match mode.as_str() {
                "confirm-worktree" => Some(RailMenuAction::RemoveWorktree),
                "confirm-branch" => Some(RailMenuAction::DeleteBranch),
                _ => None,
            };
            workspace.rail_menu = Some(RailMenuState {
                project_index: workspace.active,
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
                workspace.explorer_naming = Some(ExplorerNaming {
                    kind: NamingKind::NewFile,
                    parent: root,
                    target: None,
                    value: "new_file.rs".to_string(),
                    focus: cx.focus_handle(),
                });
            }
        }
        workspace.update_agent_destination(cx); // 宛先チップにプロジェクト/ブランチを反映
        workspace.start_watcher(cx); // アクティブプロジェクトのファイル監視（M10 watch 基盤）
        workspace.schedule_update_check(cx); // 自動アップデートの確認（M13・90s 後に背景で）
        // 開発用: SHIRUSHI_UPDATE_PROBE="x.y.z" でチップ描画を直接確認（ネット不要）。
        if let Ok(version) = std::env::var("SHIRUSHI_UPDATE_PROBE") {
            if !version.is_empty() {
                workspace.update_status = Some((
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
                    workspace
                        .agent_panel
                        .update(cx, |panel, cx| panel.set_storage(handle.clone(), cx));
                    workspace.storage = Some(handle);
                    // リモートプロジェクトの窓色をローカル DB から解決（M13 #3b）。
                    workspace.apply_remote_host_colors();
                }
                Err(error) => eprintln!("ローカル DB を開けない（hot exit 無効）: {error:#}"),
            }
        }
        // Agent パネルの通知（トースト・色リンク・statusbar ドット・M12-4/5）。
        // （window 付き subscribe は new() では張れないため、イベントは window 不要の形で処理し、
        //   タブ生成だけ pending に積んで次の描画パスで消化する）
        cx.subscribe(&workspace.agent_panel.clone(), Self::on_panel_event).detach();
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
            if let Some(slot) = self.projects.get_mut(index) {
                slot.open_files = tabs.files.clone();
                slot.active_file = tabs.active;
            }
        }
        self.open_slot_files(window, cx);
    }

    /// 複数ファイルをアクティブプロジェクトのタブとして順に開く（最後がアクティブ）。
    /// 外部起点（起動時の複数ファイル指定・開発用フック）から使う。
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        for path in paths {
            self.open_file(path, window, cx);
        }
    }

    fn active_slot(&self) -> Option<&ProjectSlot> {
        self.projects.get(self.active)
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
        if let Some(slot) = self.projects.get_mut(self.active) {
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
        let Some(worktree) = self.active_worktree() else {
            self.git_status.clear();
            self.git_changes.clear();
            self.git_history.clear();
            self.github_slug = None;
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let panel_open = self.git_panel.is_some();
        self.git_refresh_gen = self.git_refresh_gen.wrapping_add(1);
        let generation = self.git_refresh_gen;
        cx.spawn(async move |workspace, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move {
                    let status = project::git_status_on(host.as_ref(), &root);
                    let branch = project::git_current_branch_on(host.as_ref(), &root);
                    let (changes, history, slug) = if panel_open {
                        (
                            project::git_changes_on(host.as_ref(), &root),
                            project::git_log_graph_on(host.as_ref(), &root, 30),
                            project::github_slug_on(host.as_ref(), &root),
                        )
                    } else {
                        (Vec::new(), Vec::new(), None)
                    };
                    (status, branch, changes, history, slug)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if workspace.git_refresh_gen != generation {
                    return; // 古い結果（その後に別の refresh が走った）
                }
                let (status, branch, changes, history, slug) = snapshot;
                workspace.git_status = status.into_iter().collect();
                if let Some(slot) = workspace.projects.get_mut(workspace.active) {
                    slot.branch = branch;
                }
                workspace.git_changes = changes;
                workspace.git_history = history;
                workspace.github_slug = slug;
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
    /// ⎇ メニューの開閉。ブランチ/worktree の列挙は背景で集め、揃ってから開く（UI は止めない）。
    fn toggle_branch_menu(&mut self, position: Point<gpui::Pixels>, cx: &mut Context<Self>) {
        if self.branch_menu.take().is_some() {
            cx.notify();
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
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
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.branch_menu = Some(BranchMenuState { position, current, branches, worktrees });
                cx.notify();
            });
        })
        .detach();
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
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        // checkout は大きいリポジトリで秒単位になりうる → 背景 + busy 表示。
        self.git_busy = true;
        cx.notify();
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
                workspace.git_busy = false;
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
    fn open_branch_worktree(&mut self, branch: String, cx: &mut Context<Self>) {
        self.branch_menu = None;
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
        self.git_busy = true;
        cx.notify();
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
                workspace.git_busy = false;
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
    fn open_worktree_window(&mut self, path: PathBuf, branch: Option<String>, cx: &mut Context<Self>) {
        self.branch_menu = None;
        let host = match self.active_worktree() {
            Some(worktree) => worktree.host().clone(),
            None => host::LocalHost::shared(),
        };
        self.open_folder_in_rail(host, path, branch, cx);
    }

    /// ブランチ切替後などにアクティブプロジェクトを再読込（ツリー再構築・開ファイル再読込・git 更新）。
    fn reload_active_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(slot) = self.projects.get_mut(self.active) {
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
    fn toggle_git_panel(&mut self, _: &ToggleGitPanel, window: &mut Window, cx: &mut Context<Self>) {
        match self.git_panel.take() {
            Some(_) => {
                // 閉じる → エディタがあればフォーカスを戻す。
                if let Some(editor) = self.active_editor() {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            None => {
                self.show_left = true;
                self.todo_board = None; // 左カラムは排他（M12-10）
                let state =
                    GitPanelState { message: String::new(), branch_name: None, focus: cx.focus_handle() };
                window.focus(&state.focus, cx);
                self.git_panel = Some(state);
                self.refresh_git_status(cx);
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
    /// コミット（何も staged でなければ全部 stage してから）。git はフックで長引きうる → 背景 + busy。
    fn git_commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let message = self.git_panel.as_ref().map(|state| state.message.clone()).unwrap_or_default();
        if message.trim().is_empty() {
            return;
        }
        self.git_busy = true;
        cx.notify();
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
                workspace.git_busy = false;
                match result {
                    Ok(()) => {
                        if let Some(state) = workspace.git_panel.as_mut() {
                            state.message.clear();
                        }
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
    fn run_git_index_op(
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
    fn git_stage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.run_git_index_op(
            "stage".to_string(),
            move |host, root| project::stage_path_on(host.as_ref(), &root, &path),
            cx,
        );
    }

    /// 1 ファイルを unstage。
    fn git_unstage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.run_git_index_op(
            "unstage".to_string(),
            move |host, root| project::unstage_path_on(host.as_ref(), &root, &path),
            cx,
        );
    }

    /// 全変更を stage。
    fn git_stage_all(&mut self, cx: &mut Context<Self>) {
        self.run_git_index_op(
            "stage".to_string(),
            |host, root| project::stage_all_on(host.as_ref(), &root),
            cx,
        );
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
                        if let Some(state) = workspace.git_panel.as_mut() {
                            state.branch_name = None;
                        }
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
    fn delete_git_branch(&mut self, branch: String, cx: &mut Context<Self>) {
        self.branch_menu = None;
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let branch_for_msg = branch.clone();
        self.git_busy = true;
        cx.notify();
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
                workspace.git_busy = false;
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

    /// アクティブファイルの言語に合った言語サーバを（必要なら）起動する。
    /// 別プロジェクト or 別言語に移ったら張り替える。サーバ未登録の拡張子では何もしない
    /// （既存接続は温存＝別タブに戻れば診断が残る）。
    fn ensure_lsp(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let Some(path) = self
            .active_editor()
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
        self.lsp_sent_versions.clear();
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
            // capabilities から didChange の sync 種別を読む（2 = Incremental・M11-8）。
            let incremental = match init_rx.await {
                Ok(Ok(value)) => {
                    let sync = value.pointer("/capabilities/textDocumentSync");
                    let kind = sync
                        .and_then(|sync| sync.as_u64())
                        .or_else(|| sync.and_then(|sync| sync.pointer("/change")).and_then(|c| c.as_u64()));
                    kind == Some(2)
                }
                _ => false,
            };
            if workspace
                .update(cx, |ws, cx| {
                    ws.lsp_incremental_sync = incremental;
                    ws.on_lsp_initialized(cx)
                })
                .is_err()
            {
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
        let Some(editor) = self.active_editor() else {
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
            self.lsp_sent_versions.insert(path.clone(), version);
            if let Some(lsp) = &self.lsp {
                lsp.did_open(&path, language_id, version as i32, &text);
            }
        }
    }

    /// タブを閉じたら didClose を送り、送信済み version 記録も外す（稼働中サーバ担当言語のみ）。
    fn lsp_did_close(&mut self, path: &Path) {
        self.lsp_sent_versions.remove(path);
        if !self.lsp_initialized {
            return;
        }
        let closes = language_server_for(path, self.active_worktree().map(|w| w.host().is_remote()).unwrap_or(false))
            .map(|server| self.lsp_language == Some(server.language_id))
            .unwrap_or(false);
        if closes {
            if let Some(lsp) = &self.lsp {
                lsp.did_close(path);
            }
        }
    }

    /// publishDiagnostics を受けてファイル別に格納し、アクティブファイル分をエディタへ push。
    fn on_diagnostics(&mut self, params: serde_json::Value, cx: &mut Context<Self>) {
        // 生 JSON（Diagnostic[]）も保持する（⌘. の codeAction context 用・M11）。
        let raw = params.get("diagnostics").cloned().unwrap_or(serde_json::Value::Array(Vec::new()));
        let raw_uri = params.get("uri").and_then(|uri| uri.as_str()).map(str::to_string);
        let Ok(parsed) = serde_json::from_value::<lang::lsp::PublishDiagnosticsParams>(params) else {
            return;
        };
        let Some(path) = lang::lsp::uri_to_path(&parsed.uri) else {
            return;
        };
        if raw_uri.as_deref() == Some(parsed.uri.as_str()) {
            if raw.as_array().map(|array| array.is_empty()).unwrap_or(true) {
                self.raw_diagnostics.remove(&path);
            } else {
                self.raw_diagnostics.insert(path.clone(), raw);
            }
        }
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
        let Some(editor) = self.active_editor() else {
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
        // hot exit: **バッファ version が変わった時だけ**スナップショットを予約する。
        // observe は blink（530ms 毎）でも発火するので、無ガードだと 2s デバウンスが永遠に流れる。
        {
            let (path, version) = {
                let view = editor.read(cx);
                (view.buffer().path().map(Path::to_path_buf), view.buffer().version())
            };
            if let Some(path) = path {
                if self.hot_exit_versions.get(&path) != Some(&version) {
                    self.hot_exit_versions.insert(path, version);
                    self.schedule_hot_exit_snapshot(cx);
                }
            }
        }
        // blame: キャレット行が変わったら（デバウンス付きで）行末注釈を更新（M11-11）。
        self.schedule_blame(&editor, cx);
        // ⌘F が開いていれば、アクティブエディタの編集にマッチを追従させる
        // （version ガードで blink/focus の notify は素通り・再入も止まる）。
        if self.buffer_search.is_some()
            && self.active_editor().is_some_and(|active| active == editor)
        {
            self.refresh_buffer_search(false, cx);
        }
        // 初期化 + didOpen 前は didChange を送らない（さもないと ra が「initialized 前」で落ちる）。
        // observe は focus/blink 等の notify でも発火するので version 変化でのみ送る。
        if !self.lsp_initialized {
            return;
        }
        // 単一編集 + サーバが Incremental 広告 → range 差分で送る（M11-8）。
        // それ以外（複数編集/undo/redo/reload・Full サーバ）は全文。
        enum Change {
            Incremental { start: (u32, u32), end: (u32, u32), text: String },
            Full(String),
        }
        let info = {
            let view = editor.read(cx);
            let version = view.buffer().version();
            view.buffer()
                .path()
                .filter(|path| {
                    language_server_for(path, view.buffer().host().is_remote()).is_some()
                })
                // ファイル別に「送信済み version」と比較（複数タブで version 番号が衝突しても誤スキップしない）。
                .filter(|path| self.lsp_sent_versions.get(*path) != Some(&version))
                .map(|path| {
                    let change = match view.buffer().last_change() {
                        Some(edits) if edits.len() == 1 && self.lsp_incremental_sync => {
                            let (start, old, new) = (&edits[0].0, &edits[0].1, &edits[0].2);
                            // start までは編集前後で同一 → 現バッファから UTF-16 位置が取れる。
                            let (start_line, start_character) = view.lsp_position_for_offset(*start);
                            let newlines = old.matches('\n').count() as u32;
                            let end = if newlines == 0 {
                                (start_line, start_character + old.encode_utf16().count() as u32)
                            } else {
                                let tail = &old[old.rfind('\n').map(|i| i + 1).unwrap_or(0)..];
                                (start_line + newlines, tail.encode_utf16().count() as u32)
                            };
                            Change::Incremental {
                                start: (start_line, start_character),
                                end,
                                text: new.clone(),
                            }
                        }
                        _ => Change::Full(view.buffer().text()),
                    };
                    (path.to_path_buf(), version, change)
                })
        };
        if let Some((path, version, change)) = info {
            self.lsp_sent_versions.insert(path.clone(), version);
            if let Some(lsp) = &self.lsp {
                match change {
                    Change::Incremental { start, end, text } => lsp.did_change_incremental(
                        &path,
                        version as i32,
                        start.0,
                        start.1,
                        end.0,
                        end.1,
                        &text,
                    ),
                    Change::Full(text) => lsp.did_change(&path, version as i32, &text),
                }
            }
        }
    }

    /// アクティブファイルの診断件数（error, warning）。statusbar 用。
    fn active_diagnostic_counts(&self, cx: &App) -> (usize, usize) {
        let Some(editor) = self.active_editor() else {
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
        let Some(editor) = self.active_editor() else {
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
            if let Some(location) = parse_definition(&value) {
                let _ = handle.update(cx, |workspace, window, cx| {
                    workspace.record_nav_position(cx); // F12 はナビ履歴へ（⌃- で戻れる）
                    workspace.jump_to_location(
                        location.path,
                        location.position.line,
                        location.position.character,
                        window,
                        cx,
                    )
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
        let current = self.active_tab_path();
        if current.as_deref() != Some(path.as_path()) {
            // 別ファイル: 背景読み完了後のエディタへ着地（旧バッファへの誤 reveal 防止）。
            self.open_file_then(path, window, cx, move |view, cx| {
                view.reveal_lsp_position(line, character, cx)
            });
        } else if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.reveal_lsp_position(line, character, cx));
        }
        cx.notify();
    }

    /// 補完（Ctrl-Space）。カーソル位置で候補を取得しポップアップを出す。
    /// Ctrl-Space（手動トリガ）。Esc 抑止を解除して要求する。
    fn trigger_completion(&mut self, _: &TriggerCompletion, window: &mut Window, cx: &mut Context<Self>) {
        self.completion_suppressed_word = None;
        self.request_completion(window, cx);
    }

    /// LSP へ補完を要求し、応答でポップアップを出す（手動 Ctrl-Space / 自動トリガ共通）。
    /// 世代番号で連打時の古い応答を捨てる。
    fn request_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
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
        self.completion_generation = self.completion_generation.wrapping_add(1);
        let generation = self.completion_generation;
        let receiver = lsp.completion(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                if workspace.completion_generation == generation {
                    workspace.show_completion(&value, position, window, cx)
                }
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
        // 絞り込みプレフィクスとポップアップ位置は**応答時点**のキャレットを使う
        // （要求中に打たれた分・直近 paint のキャレット矩形が反映される）。要求時点の位置は fallback。
        let (prefix, fresh_position) = self
            .active_editor()
            .map(|editor| {
                let view = editor.read(cx);
                (view.identifier_prefix_at_caret().1, view.caret_window_position())
            })
            .unwrap_or_default();
        let focus = cx.focus_handle();
        let position = fresh_position
            .or(position)
            .unwrap_or_else(|| point(px(220.), px(180.)));
        let state = CompletionState { items, prefix, selected: 0, position, focus };
        // 応答時点のプレフィクスで 1 件も残らなければ出さない。
        if state.filtered().is_empty() {
            return;
        }
        window.focus(&state.focus, cx);
        self.completion = Some(state);
        cx.notify();
    }

    fn close_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.completion.take().is_none() {
            return;
        }
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn move_completion_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(state) = self.completion.as_mut() {
            let len = state.filtered().len() as isize;
            if len == 0 {
                return;
            }
            state.selected = (state.selected as isize + delta).rem_euclid(len) as usize;
            cx.notify();
        }
    }

    fn confirm_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let insert = self.completion.as_ref().and_then(|state| {
            let filtered = state.filtered();
            filtered
                .get(state.selected)
                .and_then(|&index| state.items.get(index))
                .map(|item| item.insert_text.clone())
        });
        self.completion = None;
        if let Some(editor) = self.active_editor() {
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
            "escape" => {
                // Esc = 同じ語の入力継続では自動再表示しない（語頭 offset を記憶）。
                self.completion_suppressed_word = self
                    .active_editor()
                    .map(|editor| editor.read(cx).identifier_prefix_at_caret().0);
                self.close_completion(window, cx);
            }
            "up" => self.move_completion_selection(-1, cx),
            "down" => self.move_completion_selection(1, cx),
            "enter" | "tab" => self.confirm_completion(window, cx),
            "backspace" => {
                // プレフィクスを 1 文字戻してエディタにも反映（空になったら閉じる）。
                if let Some(editor) = self.active_editor() {
                    editor.update(cx, |view, cx| view.delete_backward_char(cx));
                }
                let emptied = match self.completion.as_mut() {
                    Some(state) if !state.prefix.is_empty() => {
                        state.prefix.pop();
                        state.selected = 0;
                        state.filtered().is_empty()
                    }
                    _ => true,
                };
                if emptied {
                    self.close_completion(window, cx);
                }
                cx.notify();
            }
            _ => {
                // 印字キーは type-through: エディタへ挿入し、Typed イベント経由で
                // 絞り込み継続 / 新規トリガ / クローズが決まる（on_editor_typed）。
                let modifiers = event.keystroke.modifiers;
                let printable = !(modifiers.platform || modifiers.control || modifiers.function);
                let text = event
                    .keystroke
                    .key_char
                    .clone()
                    .filter(|text| printable && !text.is_empty() && !text.chars().any(char::is_control));
                match (text, self.active_editor()) {
                    (Some(text), Some(editor)) => {
                        editor.update(cx, |view, cx| view.insert_text(&text, cx));
                    }
                    // 印字以外（矢印+修飾・⌘系 等）は従来どおり閉じてエディタへ戻す。
                    _ => self.close_completion(window, cx),
                }
            }
        }
    }

    // ── フォーマット（⌥⇧F / 保存時・M11） ──

    fn format_document(&mut self, _: &Format, window: &mut Window, cx: &mut Context<Self>) {
        self.request_format(false, window, cx);
    }

    /// LSP フォーマットを要求して適用する。`save_after` = 適用後に保存（保存時フォーマット経路）。
    /// LSP が使えない/対応外のときは、`save_after` なら素の保存だけ行う。
    fn request_format(&mut self, save_after: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
            return;
        };
        let formattable = {
            let view = editor.read(cx);
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(Path::to_path_buf)
        };
        let (Some(path), true) = (formattable, self.lsp_initialized) else {
            if save_after {
                editor.update(cx, |view, cx| view.save_now(cx));
            }
            return;
        };
        let Some(lsp) = &self.lsp else {
            if save_after {
                editor.update(cx, |view, cx| view.save_now(cx));
            }
            return;
        };
        let tab_size = settings::get(cx).tab_size as u32;
        let receiver = lsp.formatting(&path, tab_size);
        cx.spawn(async move |_workspace, cx| {
            let response = receiver.await;
            let _ = handle.update(cx, |workspace, _window, cx| {
                let Some(editor) = workspace.active_editor() else {
                    return;
                };
                // 応答が来た時に別ファイルへ移っていたら適用しない（誤爆防止）。
                if editor.read(cx).buffer().path() != Some(path.as_path()) {
                    return;
                }
                if let Ok(Ok(value)) = response {
                    let edits = parse_text_edits(&value);
                    if !edits.is_empty() {
                        editor.update(cx, |view, cx| {
                            let byte_edits = edits
                                .iter()
                                .map(|edit| {
                                    (
                                        view.lsp_range_to_bytes(
                                            edit.range.start.line,
                                            edit.range.start.character,
                                            edit.range.end.line,
                                            edit.range.end.character,
                                        ),
                                        edit.new_text.clone(),
                                    )
                                })
                                .collect();
                            view.apply_lsp_edits(byte_edits, cx);
                        });
                    }
                }
                if save_after {
                    editor.update(cx, |view, cx| view.save_now(cx));
                }
            });
        })
        .detach();
    }

    /// ⌘S。`format_on_save` が有効ならフォーマット → 保存、無効なら即保存（どちらも背景書き込み）。
    fn save_active(&mut self, _: &SaveActive, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        if settings::get(cx).format_on_save {
            self.request_format(true, window, cx);
        } else {
            editor.update(cx, |view, cx| view.save_now(cx));
        }
    }

    // ── rename（F2・M11） ──

    fn open_rename(&mut self, _: &Rename, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        // LSP 対応ファイルのみ。初期値はキャレット下の単語。
        let seed = {
            let view = editor.read(cx);
            if view
                .buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .is_none()
                || !self.lsp_initialized
            {
                return;
            }
            let snapshot = view.buffer().snapshot();
            let head = view.buffer().selections().first().map(|s| s.head).unwrap_or(0);
            snapshot
                .word_range_at(head)
                .map(|range| view.buffer().text_range(range))
                .unwrap_or_default()
        };
        if seed.is_empty() {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.rename_input = Some((seed, focus));
        cx.notify();
    }

    fn close_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename_input.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    fn on_rename_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_rename(window, cx),
            "enter" => {
                let new_name = self
                    .rename_input
                    .as_ref()
                    .map(|(value, _)| value.trim().to_string())
                    .unwrap_or_default();
                self.close_rename(window, cx);
                if !new_name.is_empty() {
                    self.perform_rename(new_name, window, cx);
                }
            }
            "backspace" => {
                if let Some((value, _)) = self.rename_input.as_mut() {
                    value.pop();
                    cx.notify();
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }
                if let Some((value, _)) = self.rename_input.as_mut() {
                    value.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    /// rename を LSP へ要求し、WorkspaceEdit を全ファイルへ適用する。
    /// 開いているタブはバッファへ（dirty のまま）・未オープンはディスクへ直書き。
    fn perform_rename(&mut self, new_name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let (path, line, character) = {
            let view = editor.read(cx);
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            let (line, character) = view.cursor_lsp_position();
            (path, line, character)
        };
        let Some(lsp) = &self.lsp else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let receiver = lsp.rename(&path, line, character, &new_name);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, _window, cx| {
                workspace.apply_workspace_edit(&value, cx);
            });
        })
        .detach();
    }

    /// WorkspaceEdit を適用する共通経路（rename / code actions・M11）。
    fn apply_workspace_edit(&mut self, value: &serde_json::Value, cx: &mut Context<Self>) {
        let by_file = parse_workspace_edit(value);
        if by_file.is_empty() {
            return;
        }
        let mut buffers = 0usize;
        let mut disk_files = 0usize;
        let mut failed = 0usize;
        for file_edits in by_file {
            let path = file_edits.path;
            let edits = file_edits.edits;
            if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path) {
                let editor = tab.editor.clone();
                editor.update(cx, |view, cx| {
                    let byte_edits = edits
                        .iter()
                        .map(|edit| {
                            (
                                view.lsp_range_to_bytes(
                                    edit.range.start.line,
                                    edit.range.start.character,
                                    edit.range.end.line,
                                    edit.range.end.character,
                                ),
                                edit.new_text.clone(),
                            )
                        })
                        .collect();
                    view.apply_lsp_edits(byte_edits, cx);
                });
                buffers += 1;
                continue;
            }
            // 未オープン: ディスクへ直書き（読み → 適用 → 書き。UI スレッドだが local テキストで軽量。
            // remote は rename 要求自体が remote LSP 側なのでここへは来ない想定・来ても host 経由）。
            let Some(worktree) = self.active_worktree() else {
                continue;
            };
            let host = worktree.host().clone();
            match host.read_file(&path) {
                Ok(content) => match String::from_utf8(content.bytes) {
                    Ok(text) => {
                        let updated = apply_text_edits_to_string(&text, &edits);
                        match host.write_file(
                            &path,
                            updated.as_bytes(),
                            host::WriteCondition::Matches(content.revision),
                        ) {
                            Ok(_) => disk_files += 1,
                            Err(error) => {
                                failed += 1;
                                eprintln!("rename の書き込みに失敗 {}: {error:#}", path.display());
                            }
                        }
                    }
                    Err(_) => failed += 1,
                },
                Err(error) => {
                    failed += 1;
                    eprintln!("rename の読み込みに失敗 {}: {error:#}", path.display());
                }
            }
        }
        eprintln!("rename: バッファ {buffers}・ディスク {disk_files}・失敗 {failed}");
        self.refresh_git_status(cx);
        cx.notify();
    }

    /// F2 のミニ入力（キャレット近く…は座標が要るので v1 は中央上・⌃G と同型）。
    fn render_rename_input(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (value, focus) = self.rename_input.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let display: SharedString = SharedString::from(value.clone());
        Some(
            div()
                .absolute()
                .top(px(96.))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(320.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(30.))
                        .px(px(10.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(accent)
                        .rounded(px(8.))
                        .shadow(vec![gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4))
                            .blur_radius(px(16.))])
                        .track_focus(focus)
                        .on_key_down(cx.listener(Self::on_rename_key_down))
                        .text_size(px(12.5))
                        .text_color(theme.fg0)
                        .child(div().flex_none().text_size(px(11.)).text_color(theme.fg2).child(SharedString::from(i18n::t!("rename.label"))))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(display))
                        .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent)),
                )
                .into_any_element(),
        )
    }

    // ── ⌘I インライン編集（M12-8） ──

    /// ⌘I: 選択範囲（無ければ現在行）を対象にインライン編集を開く。
    /// 選択+指示 → `claude -p` で書き換え → その場 diff → accept/reject（チャットへ行かない最短経路）。
    /// ターミナルにフォーカスがあれば同型の「自然言語 → コマンド生成」になる。
    fn open_inline_edit(&mut self, _: &InlineEdit, window: &mut Window, cx: &mut Context<Self>) {
        let terminal_focused = self.terminal_dock.read(cx).is_any_focused(window, cx);
        let target = if terminal_focused {
            InlineEditTarget::Terminal
        } else {
            let Some(editor) = self.active_editor() else {
                return;
            };
            let (range, old_text, buffer_version) = {
                let view = editor.read(cx);
                let buffer = view.buffer();
                if buffer.is_read_only() {
                    return; // diff タブ等は対象外
                }
                let selection =
                    buffer.selections().first().copied().unwrap_or(Selection::cursor(0));
                let mut range = selection.range();
                let snapshot = buffer.snapshot();
                if range.is_empty() {
                    // 選択が無ければ現在行全体（改行込み）を対象にする。
                    let point = snapshot.byte_to_point(range.start);
                    let start = snapshot.point_to_byte(editor_core::Point::new(point.row, 0));
                    let end = if point.row + 1 < snapshot.line_count() {
                        snapshot.point_to_byte(editor_core::Point::new(point.row + 1, 0))
                    } else {
                        snapshot.len_bytes()
                    };
                    range = start..end;
                }
                (range.clone(), buffer.text_range(range), buffer.version())
            };
            if old_text.trim().is_empty() {
                self.push_toast(
                    SharedString::from(i18n::t!("inline.empty_target")),
                    self.accent(),
                    cx,
                );
                return;
            }
            InlineEditTarget::Editor { range, old_text, buffer_version }
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.inline_edit = Some(InlineEditState {
            instruction: String::new(),
            focus,
            target,
            busy: false,
            proposal: None,
            generation: 0,
            error: None,
        });
        cx.notify();
    }

    fn close_inline_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.inline_edit.take() else {
            return;
        };
        // フォーカスを元の場所（エディタ / ターミナル）へ返す。
        match state.target {
            InlineEditTarget::Editor { .. } => {
                if let Some(editor) = self.active_editor() {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            InlineEditTarget::Terminal => {
                self.terminal_dock
                    .update(cx, |dock, cx| dock.focus_active_if_present(window, cx));
            }
        }
        cx.notify();
    }

    /// Enter: 指示を `claude -p` へ（背景実行・世代ガード付き）。
    fn execute_inline_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(state) = self.inline_edit.as_mut() else {
            return;
        };
        if state.busy {
            return;
        }
        let instruction = state.instruction.trim().to_string();
        if instruction.is_empty() {
            return;
        }
        state.busy = true;
        state.error = None;
        state.proposal = None;
        state.generation += 1;
        let generation = state.generation;
        let old_text = match &state.target {
            InlineEditTarget::Editor { old_text, .. } => Some(old_text.clone()),
            InlineEditTarget::Terminal => None,
        };
        cx.notify();
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match old_text {
                        Some(old_text) => project::inline_rewrite_on(
                            host.as_ref(),
                            &root,
                            &instruction,
                            &old_text,
                        ),
                        None => project::inline_command_on(host.as_ref(), &root, &instruction),
                    }
                })
                .await;
            let _ = handle.update(cx, |workspace, _window, cx| {
                let Some(state) = workspace.inline_edit.as_mut() else {
                    return; // Esc で閉じた後に届いた古い結果は捨てる
                };
                if state.generation != generation {
                    return;
                }
                state.busy = false;
                match result {
                    Ok(text) => state.proposal = Some(text),
                    Err(error) => state.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 提案を適用。エディタ = 1 Transaction 置換（⌘Z 一発で戻る・version 不一致は安全側で破棄）。
    /// ターミナル = 生成コマンドを入力行へ挿入（実行はユーザーの Enter に委ねる）。
    fn accept_inline_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.inline_edit.take() else {
            return;
        };
        let Some(proposal) = state.proposal else {
            self.inline_edit = Some(state); // 提案が無いのに呼ばれたら何もしない
            return;
        };
        match &state.target {
            InlineEditTarget::Editor { range, buffer_version, .. } => {
                let Some(editor) = self.active_editor() else {
                    return;
                };
                let applied = editor.update(cx, |view, cx| {
                    if view.buffer().version() != *buffer_version {
                        return false;
                    }
                    view.apply_lsp_edits(vec![(range.clone(), proposal)], cx);
                    true
                });
                if !applied {
                    self.push_toast(
                        SharedString::from(i18n::t!("inline.buffer_changed")),
                        self.accent(),
                        cx,
                    );
                }
                if let Some(editor) = self.active_editor() {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            InlineEditTarget::Terminal => {
                self.terminal_dock
                    .update(cx, |dock, cx| dock.insert_text(&proposal, window, cx));
            }
        }
        cx.notify();
    }

    fn on_inline_edit_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let has_proposal = self
            .inline_edit
            .as_ref()
            .is_some_and(|state| state.proposal.is_some());
        match event.keystroke.key.as_str() {
            "escape" => self.close_inline_edit(window, cx),
            "enter" => {
                if has_proposal {
                    self.accept_inline_edit(window, cx);
                } else {
                    self.execute_inline_edit(window, cx);
                }
            }
            "backspace" => {
                if let Some(state) = self.inline_edit.as_mut() {
                    if state.proposal.is_some() {
                        // 提案表示中の編集は「指示を直してやり直す」= 提案を捨てて入力へ戻る。
                        state.proposal = None;
                    }
                    state.instruction.pop();
                    cx.notify();
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }
                if let Some(state) = self.inline_edit.as_mut() {
                    if state.proposal.is_some() {
                        state.proposal = None;
                    }
                    state.instruction.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    /// ⌘I のオーバーレイ（入力バー + 状態行 + diff プレビュー）。rename と同じ中央上配置。
    fn render_inline_edit(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.inline_edit.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let display: SharedString = SharedString::from(state.instruction.clone());
        let placeholder = state.instruction.is_empty();
        let input_row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(30.))
            .px(px(10.))
            .text_size(px(12.5))
            .text_color(theme.fg0)
            .child(
                div().flex_none().text_size(px(11.)).text_color(accent).child(
                    SharedString::from(match &state.target {
                        InlineEditTarget::Editor { .. } => i18n::t!("inline.title"),
                        InlineEditTarget::Terminal => i18n::t!("inline.title_terminal"),
                    }),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(placeholder, |element| {
                        element.text_color(theme.fg2).child(SharedString::from(i18n::t!(
                            "inline.instruction_placeholder"
                        )))
                    })
                    .when(!placeholder, |element| element.child(display)),
            )
            .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent));
        let mut card = div()
            .w(px(560.))
            .flex()
            .flex_col()
            .bg(theme.bg2)
            .border_1()
            .border_color(accent)
            .rounded(px(8.))
            .shadow(vec![gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4))
                .blur_radius(px(16.))])
            .track_focus(&state.focus)
            .on_key_down(cx.listener(Self::on_inline_edit_key_down))
            .child(input_row);
        if state.busy {
            card = card.child(
                div()
                    .px(px(10.))
                    .pb(px(8.))
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("inline.busy"))),
            );
        }
        if let Some(error) = &state.error {
            card = card.child(
                div()
                    .px(px(10.))
                    .pb(px(8.))
                    .text_size(px(11.))
                    .text_color(theme.err)
                    .child(SharedString::from(error.clone())),
            );
        }
        if let Some(proposal) = &state.proposal {
            // エディタ = その場 diff（- 旧 / + 新・中央省略）/ ターミナル = 生成コマンド 1 行。
            let preview_lines = match &state.target {
                InlineEditTarget::Editor { old_text, .. } => {
                    inline_edit_diff_lines(old_text, proposal, 14)
                }
                InlineEditTarget::Terminal => vec![format!("$ {proposal}")],
            };
            let mut body = div()
                .flex()
                .flex_col()
                .px(px(10.))
                .py(px(6.))
                .border_t_1()
                .border_color(theme.border)
                .font_family("Menlo")
                .text_size(px(11.))
                .max_h(px(240.))
                .overflow_hidden();
            for line in preview_lines {
                let color = match line.chars().next() {
                    Some('+') => theme.ok,
                    Some('-') => theme.err,
                    Some('$') => theme.fg0,
                    _ => theme.fg2,
                };
                body = body.child(
                    div().whitespace_nowrap().text_color(color).child(SharedString::from(line)),
                );
            }
            let actions = div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .px(px(10.))
                .py(px(7.))
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .id("inline-accept")
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .bg(accent)
                        .text_size(px(11.5))
                        .text_color(gpui::white())
                        .cursor_pointer()
                        .child(SharedString::from(i18n::t!("inline.accept")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.accept_inline_edit(window, cx)),
                        ),
                )
                .child(
                    div()
                        .id("inline-reject")
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .child(SharedString::from(i18n::t!("inline.reject")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_inline_edit(window, cx)),
                        ),
                );
            card = card.child(body).child(actions);
        }
        Some(
            div()
                .absolute()
                .top(px(96.))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .child(card)
                .into_any_element(),
        )
    }

    // ── Todo ボード（M12-10） ──

    /// レール ☑ / アクション: 板の表示切替。開く時に読み込み、git パネルとは排他。
    fn toggle_todo_board(&mut self, _: &ToggleTodoBoard, _window: &mut Window, cx: &mut Context<Self>) {
        if self.todo_board.take().is_some() {
            cx.notify();
            return;
        }
        self.git_panel = None;
        self.show_left = true;
        self.todo_board = Some(TodoBoardState {
            items: Vec::new(),
            plan_busy: false,
            running: HashMap::new(),
            add_input: None,
        });
        self.reload_todo_board(cx);
        cx.notify();
    }

    /// 板を `.shirushi/todos.md` から読み直す（背景・最新勝ち）。
    /// 開いた時・チェック書換後・watch 検知・TurnEnded で呼ぶ＝どの書き手の変更も追従する。
    fn reload_todo_board(&mut self, cx: &mut Context<Self>) {
        if self.todo_board.is_none() {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.spawn(async move |workspace, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { project::todos::read_todos_on(host.as_ref(), &root) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Some(board) = workspace.todo_board.as_mut() {
                    board.items = items;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// チェッククリック: 該当行の `[ ]`↔`[x]` をファイル書き換え（他の書き手と同じ経路）。
    fn toggle_todo_item(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::toggle_todo_on(host.as_ref(), &root, line) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
                workspace.reload_todo_board(cx);
            });
        })
        .detach();
    }

    /// ▶: 項目をアクティブスレッドへ送る。末尾に「完了したら板をチェックせよ」を自動付与し、
    /// エージェント自身が todos.md を書き換える → watch が板へ反映（= チェックがひとりでに入る）。
    fn send_todo_to_agent(&mut self, line: usize, text: String, cx: &mut Context<Self>) {
        let prompt = i18n::t!("todos.send_prompt", "text" => text);
        let color = self.agent_panel.read(cx).active_color();
        self.agent_panel.update(cx, |panel, cx| panel.send_prompt_text(prompt, cx));
        self.show_right = true;
        if let Some(board) = self.todo_board.as_mut() {
            board.running.insert(line, color);
        }
        cx.notify();
    }

    /// ✨今日の計画: ROADMAP/git status/未消化を `claude -p` に渡して下書きを板へ追記（M12-10）。
    fn run_daily_plan(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        if self.todo_board.as_ref().is_some_and(|board| board.plan_busy) {
            return;
        }
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        if let Some(board) = self.todo_board.as_mut() {
            board.plan_busy = true;
        }
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::daily_plan_on(host.as_ref(), &root) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Some(board) = workspace.todo_board.as_mut() {
                    board.plan_busy = false;
                }
                match result {
                    Ok(count) => workspace.push_toast(
                        SharedString::from(i18n::t!("todos.plan_added", "count" => count)),
                        workspace.accent(),
                        cx,
                    ),
                    Err(error) => workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    ),
                }
                workspace.reload_todo_board(cx);
            });
        })
        .detach();
    }

    /// ＋ でタスク追加の入力を開く（IME 正しい EditorView::plain・Enter 確定 / Esc 取消）。
    fn start_add_todo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.todo_board.is_none() {
            return;
        }
        let theme = self.theme.clone();
        let accent = self.accent();
        let editor = cx.new(|cx| EditorView::plain(theme, accent, true, cx));
        cx.subscribe(&editor, |workspace, _editor, event, cx| match event {
            ComposerEvent::Submit => workspace.confirm_add_todo(cx),
        })
        .detach();
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        if let Some(board) = self.todo_board.as_mut() {
            board.add_input = Some(editor);
        }
        cx.notify();
    }

    /// タスク追加を確定（Enter）。空白は無視。今日の見出し下へ `add_todo_on` で追記し板を読み直す。
    fn confirm_add_todo(&mut self, cx: &mut Context<Self>) {
        let text = self
            .todo_board
            .as_ref()
            .and_then(|board| board.add_input.as_ref())
            .map(|editor| editor.read(cx).plain_text().trim().to_string())
            .unwrap_or_default();
        if let Some(board) = self.todo_board.as_mut() {
            board.add_input = None; // 追記中は閉じる（連続追加は再度 ＋）
        }
        if text.is_empty() {
            cx.notify();
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let host = worktree.host().clone();
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::todos::add_todo_on(host.as_ref(), &root, &text) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
                workspace.reload_todo_board(cx);
            });
        })
        .detach();
    }

    /// タスク追加を取り消す（Esc）。入力内容は破棄する。
    fn cancel_add_todo(&mut self, cx: &mut Context<Self>) {
        if let Some(board) = self.todo_board.as_mut() {
            if board.add_input.take().is_some() {
                cx.notify();
            }
        }
    }

    /// 左カラムの Todo ボード（explorer/git と同じ幅・M12-10）。
    fn render_todo_board(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let Some(board) = self.todo_board.as_ref() else {
            return div().into_any_element();
        };
        let open_count = board.items.iter().filter(|item| !item.done).count();
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(34.))
            .px(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.fg1)
                    .child(SharedString::from(i18n::t!("todos.title"))),
            )
            .child(div().text_size(px(10.5)).text_color(theme.fg2).child(format!("{open_count}")))
            .child(div().flex_1())
            // ✨ 今日の計画（claude -p）。
            .child(
                div()
                    .id("todos-plan")
                    .px(px(7.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .text_color(if board.plan_busy { theme.fg2 } else { accent })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .child(if board.plan_busy {
                        SharedString::from(i18n::t!("todos.plan_busy"))
                    } else {
                        SharedString::from(i18n::t!("todos.plan"))
                    })
                    .tooltip(Tooltip::text(i18n::t!("todos.plan_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.run_daily_plan(cx)),
                    ),
            )
            // ＋ タスクを追加（インライン入力・#todo-add）。
            .child(
                div()
                    .id("todos-add")
                    .px(px(7.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(13.))
                    .text_color(accent)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .child("＋")
                    .tooltip(Tooltip::text(i18n::t!("todos.add_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.start_add_todo(window, cx)),
                    ),
            );
        let mut list = div().flex_1().flex().flex_col().overflow_hidden().py(px(4.));
        if board.items.is_empty() {
            list = list.child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("todos.empty"))),
            );
        }
        let mut last_section: Option<String> = None;
        for item in &board.items {
            // 見出し（日付）はセクションが変わった時だけ描く。
            if item.section != last_section {
                if let Some(section) = &item.section {
                    list = list.child(
                        div()
                            .px(px(10.))
                            .pt(px(8.))
                            .pb(px(2.))
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(section.clone())),
                    );
                }
                last_section = item.section.clone();
            }
            let line = item.line;
            let text = item.text.clone();
            let done = item.done;
            let running_color = board.running.get(&line).copied();
            let mark = if done { "☑" } else { "☐" };
            let mut row = div()
                .id(("todo-item", line))
                .group("todo-row")
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(10.))
                .py(px(3.))
                .text_size(px(12.))
                .hover(|style| style.bg(theme.bg2))
                // ☐/☑（クリック = ファイル書き換え）。
                .child(
                    div()
                        .id(("todo-check", line))
                        .flex_none()
                        .text_color(if done { theme.ok } else { theme.fg2 })
                        .cursor_pointer()
                        .child(mark)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| this.toggle_todo_item(line, cx)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(if done { theme.fg2 } else { theme.fg0 })
                        .child(SharedString::from(item.text.clone())),
                );
            // 実行中はスレッド色 pulse・そうでなければ hover で ▶。
            if let Some(color) = running_color {
                row = row.child(beacon_dot(("todo-running", line), color, true));
            } else if !done {
                row = row.child(
                    div()
                        .id(("todo-send", line))
                        .flex_none()
                        .invisible()
                        .group_hover("todo-row", |style| style.visible())
                        .text_size(px(11.))
                        .text_color(accent)
                        .cursor_pointer()
                        .child("▶")
                        .tooltip(Tooltip::text(i18n::t!("todos.send_tip"), theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.send_todo_to_agent(line, text.clone(), cx)
                            }),
                        ),
                );
            }
            list = list.child(row);
        }
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
            .child(header)
            // ＋ の追加入力（IME 正しい EditorView・Enter 確定 / Esc 取消）。
            .children(board.add_input.clone().map(|editor| {
                div()
                    .flex_none()
                    .mx(px(8.))
                    .my(px(4.))
                    .px(px(6.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(accent)
                    .bg(theme.bg1)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _window, cx| {
                        if event.keystroke.key.as_str() == "escape" {
                            this.cancel_add_todo(cx);
                        }
                    }))
                    .child(editor)
            }))
            .child(list)
            .child(self.left_dock_resize_handle(cx))
            .into_any_element()
    }

    // ── diff タブ（M11-9）と hunk 操作（M11-10） ──

    /// アクティブファイルの HEAD vs バッファ unified diff を一時タブで開く。
    fn open_diff_tab(&mut self, _: &OpenDiff, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let (path, current) = {
            let view = editor.read(cx);
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            (path, view.buffer().text())
        };
        self.open_diff_tab_for(path, Some(current), window, cx);
    }

    /// 指定ファイルの diff タブを開く（git パネル行から）。`current` が None ならディスクの内容を使う。
    fn open_diff_tab_for(
        &mut self,
        path: PathBuf,
        current: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            let (diff_text, path) = cx
                .background_executor()
                .spawn(async move {
                    let current = current.or_else(|| {
                        host.read_file(&path)
                            .ok()
                            .and_then(|content| String::from_utf8(content.bytes).ok())
                    });
                    let diff = current
                        .as_deref()
                        .and_then(|current| project::unified_diff_on(host.as_ref(), &path, current));
                    (diff, path)
                })
                .await;
            let Some(diff_text) = diff_text else {
                eprintln!("diff: 差分なし（HEAD と同一） {}", path.display());
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                let title = path.with_file_name(format!("{name} ⇄ HEAD"));
                let mut buffer = Buffer::from_str(&diff_text);
                buffer.set_read_only(true);
                workspace.open_transient_tab(title, buffer, window, cx);
            });
        })
        .detach();
    }

    /// diff タブ内で次/前の hunk ヘッダ（@@ 行）へ移動する（F7/⇧F7）。
    fn step_hunk_header(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |view, cx| {
            let snapshot = view.buffer().snapshot();
            let current_row = snapshot
                .byte_to_point(view.buffer().selections().first().map(|s| s.head).unwrap_or(0))
                .row;
            let rows: Vec<usize> = (0..snapshot.line_count())
                .filter(|row| snapshot.line_text(*row).starts_with("@@"))
                .collect();
            if rows.is_empty() {
                return;
            }
            let target = if delta > 0 {
                rows.iter().copied().find(|row| *row > current_row).unwrap_or(rows[0])
            } else {
                rows.iter().rev().copied().find(|row| *row < current_row).unwrap_or(*rows.last().unwrap())
            };
            view.reveal_position(target, 0, cx);
        });
    }

    /// gutter の diff バークリック（hunk ポップオーバー）。
    fn on_hunk_clicked(
        &mut self,
        hunk: project::DiffHunk,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.hunk_menu = Some((hunk, position));
        cx.notify();
    }

    /// hunk を stage する（この hunk だけ index へ）。
    fn stage_hunk(&mut self, hunk: project::DiffHunk, cx: &mut Context<Self>) {
        self.hunk_menu = None;
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let (path, current) = {
            let view = editor.read(cx);
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            (path, view.buffer().text())
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let head = project::head_text_on(host.as_ref(), &path).unwrap_or_default();
                    let repo = path
                        .ancestors()
                        .find(|dir| dir.join(".git").exists())
                        .map(Path::to_path_buf)
                        .unwrap_or(root);
                    let relative = path
                        .strip_prefix(&repo)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    let head_lines: Vec<&str> = head.lines().collect();
                    let current_lines: Vec<&str> = current.lines().collect();
                    let patch =
                        project::hunk_patch_text(&relative, &head_lines, &current_lines, &hunk);
                    project::apply_patch_to_index_on(host.as_ref(), &repo, &patch)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    eprintln!("hunk stage に失敗: {error:#}");
                }
                workspace.refresh_git_status(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// hunk を巻き戻す（バッファ上で HEAD の内容に戻す・undo 可）。
    fn revert_hunk(&mut self, hunk: project::DiffHunk, cx: &mut Context<Self>) {
        self.hunk_menu = None;
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        editor.update(cx, |view, cx| {
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            let head = project::head_text_on(host.as_ref(), &path).unwrap_or_default();
            let head_lines: Vec<&str> = head.lines().collect();
            let replacement: String = head_lines
                .iter()
                .skip(hunk.old_range.start as usize)
                .take(hunk.old_range.len())
                .map(|line| format!("{line}\n"))
                .collect();
            let snapshot = view.buffer().snapshot();
            let start_row = hunk.new_range.start as usize;
            let end_row = hunk.new_range.end as usize;
            let start = snapshot.point_to_byte(editor_core::Point::new(start_row.min(snapshot.line_count().saturating_sub(1)), 0));
            let end = if hunk.new_range.is_empty() {
                start // 削除 hunk: その位置に HEAD の行を挿入
            } else if end_row < snapshot.line_count() {
                snapshot.point_to_byte(editor_core::Point::new(end_row, 0))
            } else {
                view.buffer().len_bytes()
            };
            view.replace_ranges(&[start..end], &replacement, cx);
        });
    }

    /// hunk のバッファ側テキストをコピーする。
    fn copy_hunk(&mut self, hunk: project::DiffHunk, cx: &mut Context<Self>) {
        self.hunk_menu = None;
        let Some(editor) = self.active_editor() else {
            return;
        };
        let text = {
            let view = editor.read(cx);
            let snapshot = view.buffer().snapshot();
            (hunk.new_range.start as usize..hunk.new_range.end as usize)
                .filter(|row| *row < snapshot.line_count())
                .map(|row| format!("{}\n", snapshot.line_text(row)))
                .collect::<String>()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.notify();
    }

    /// hunk 操作ポップオーバー（gutter クリックで開く・M11-10）。
    fn render_hunk_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (hunk, position) = self.hunk_menu.clone()?;
        let theme = self.theme.clone();
        let item = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .px(px(9.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(label))
        };
        let stage_hunk = hunk.clone();
        let revert_hunk_data = hunk.clone();
        let copy_hunk_data = hunk;
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.hunk_menu = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(position.x + px(8.))
                        .top(position.y)
                        .w(px(220.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .p(px(4.))
                        .shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
                        ])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(item("hunk-stage", i18n::t!("hunk.stage")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| this.stage_hunk(stage_hunk.clone(), cx)),
                        ))
                        .child(item("hunk-revert", i18n::t!("hunk.revert")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.revert_hunk(revert_hunk_data.clone(), cx)
                            }),
                        ))
                        .child(item("hunk-copy", i18n::t!("hunk.copy")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| this.copy_hunk(copy_hunk_data.clone(), cx)),
                        ))
                        .child(item("hunk-diff", i18n::t!("hunk.open_diff")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.hunk_menu = None;
                                this.open_diff_tab(&OpenDiff, window, cx)
                            }),
                        )),
                )
                .into_any_element(),
        )
    }

    // ── blame（キャレット行の行末に 由来 を dim 表示・M11-11） ──

    /// キャレット行が変わっていたら 400ms デバウンスで `git blame -L` を背景実行し、
    /// 行末注釈へ反映する。dirty バッファでは HEAD 基準の近似（行ずれ許容）。
    fn schedule_blame(&mut self, editor: &Entity<EditorView>, cx: &mut Context<Self>) {
        if self.active_editor().as_ref() != Some(editor) {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let target = {
            let view = editor.read(cx);
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                // 無題/一時タブは注釈なし。
                return;
            };
            let head = view.buffer().selections().first().map(|s| s.head).unwrap_or(0);
            let row = view.buffer().snapshot().byte_to_point(head).row;
            (path, row)
        };
        if self.last_blame_target.as_ref() == Some(&target) {
            return; // 同じ行（blink や横移動）では再計算しない
        }
        self.last_blame_target = Some(target.clone());
        self.blame_gen = self.blame_gen.wrapping_add(1);
        let generation = self.blame_gen;
        let host = worktree.host().clone();
        let editor = editor.clone();
        let (path, row) = target;
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            let latest = workspace
                .update(cx, |workspace, _| workspace.blame_gen == generation)
                .unwrap_or(false);
            if !latest {
                return;
            }
            let annotation = cx
                .background_executor()
                .spawn(async move { project::blame_line_on(host.as_ref(), &path, row as u32 + 1) })
                .await;
            let still_latest = workspace
                .update(cx, |workspace, _| workspace.blame_gen == generation)
                .unwrap_or(false);
            if !still_latest {
                return;
            }
            let _ = editor.update(cx, |view, cx| {
                view.set_line_annotation(annotation.map(|text| (row, SharedString::from(text))), cx);
            });
        })
        .detach();
    }

    // ── シンボル（⌘⇧O アウトライン / ⌘T ワークスペース・M11） ──

    /// ⌘⇧O: tree-sitter アウトラインを Picker で（LSP 不要・対応言語のみ）。
    fn open_outline(&mut self, _: &OutlineSymbols, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let items = {
            let view = editor.read(cx);
            let Some(extension) = view
                .buffer()
                .path()
                .and_then(|path| path.extension())
                .and_then(|extension| extension.to_str())
            else {
                return;
            };
            lang::outline(&view.buffer().text(), extension)
        };
        if items.is_empty() {
            return;
        }
        self.picker_symbol_rows = items.iter().map(|item| item.row).collect();
        let picker_items = items
            .iter()
            .enumerate()
            .map(|(id, item)| {
                PickerItem::new(id, format!("{} {}", item.kind, item.name))
                    .with_detail(format!("{}", item.row + 1))
            })
            .collect();
        self.open_picker(PickerMode::Symbols, i18n::t!("symbols.outline"), picker_items, window, cx);
    }

    /// ⌘T: LSP workspace/symbol を Picker で（空クエリの初期一覧 → Picker の fuzzy で絞る）。
    fn open_workspace_symbols(&mut self, _: &WorkspaceSymbols, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        if !self.lsp_initialized {
            return;
        }
        let Some(lsp) = &self.lsp else {
            return;
        };
        let receiver = lsp.workspace_symbols("");
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                let Some(array) = value.as_array() else {
                    return;
                };
                let mut locations = Vec::new();
                let mut items = Vec::new();
                for (id, symbol) in array.iter().take(200).enumerate() {
                    let Some(name) = symbol.get("name").and_then(|name| name.as_str()) else {
                        continue;
                    };
                    let Some(uri) = symbol.pointer("/location/uri").and_then(|uri| uri.as_str())
                    else {
                        continue;
                    };
                    let Some(path) = lang::lsp::uri_to_path(uri) else {
                        continue;
                    };
                    let line = symbol
                        .pointer("/location/range/start/line")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as u32;
                    let character = symbol
                        .pointer("/location/range/start/character")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as u32;
                    let container = symbol
                        .get("containerName")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    locations.push((path.clone(), line, character));
                    let file = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default();
                    items.push(
                        PickerItem::new(id, name.to_string())
                            .with_detail(if container.is_empty() { file } else { format!("{container} — {file}") }),
                    );
                }
                if items.is_empty() {
                    return;
                }
                workspace.picker_workspace_symbols = locations;
                workspace.open_picker(
                    PickerMode::WorkspaceSymbols,
                    i18n::t!("symbols.workspace"),
                    items,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    // ── 診断一覧 + F8（M11） ──

    /// F8/⇧F8: アクティブファイルの次/前の診断行へ。
    fn step_diagnostic(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(path) = self.active_tab_path() else {
            return;
        };
        let Some(entries) = self.diagnostics.get(&path) else {
            return;
        };
        let mut rows: Vec<u32> = entries.iter().map(|(line, _)| *line).collect();
        rows.sort_unstable();
        rows.dedup();
        if rows.is_empty() {
            return;
        }
        let current_row = {
            let view = editor.read(cx);
            let head = view.buffer().selections().first().map(|s| s.head).unwrap_or(0);
            view.buffer().snapshot().byte_to_point(head).row as u32
        };
        let target = if delta > 0 {
            rows.iter().copied().find(|row| *row > current_row).unwrap_or(rows[0])
        } else {
            rows.iter().rev().copied().find(|row| *row < current_row).unwrap_or(*rows.last().unwrap())
        };
        editor.update(cx, |view, cx| view.reveal_position(target as usize, 0, cx));
    }

    fn next_diagnostic(&mut self, _: &NextDiagnostic, _: &mut Window, cx: &mut Context<Self>) {
        self.step_diagnostic(1, cx);
    }

    fn prev_diagnostic(&mut self, _: &PrevDiagnostic, _: &mut Window, cx: &mut Context<Self>) {
        self.step_diagnostic(-1, cx);
    }

    /// statusbar の ✗▲ クリック: 全ファイルの診断を検索パネル UI で一覧（メッセージがプレビュー行）。
    fn open_diagnostics_panel(&mut self, _: &DiagnosticsPanel, window: &mut Window, cx: &mut Context<Self>) {
        let mut results: Vec<search::FileMatch> = Vec::new();
        let mut files: Vec<&PathBuf> = self.raw_diagnostics.keys().collect();
        files.sort();
        for path in files {
            let Some(array) = self.raw_diagnostics.get(path).and_then(|value| value.as_array())
            else {
                continue;
            };
            let mut matches: Vec<search::Match> = array
                .iter()
                .filter_map(|diagnostic| {
                    let line = diagnostic.pointer("/range/start/line")?.as_u64()? as usize;
                    let message = diagnostic.get("message")?.as_str()?;
                    let severity = diagnostic
                        .get("severity")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(1);
                    let icon = if severity == 1 { "✗" } else { "▲" };
                    Some(search::Match {
                        line,
                        column: 0,
                        byte_range: 0..0,
                        line_text: format!("{icon} {}", message.lines().next().unwrap_or("")),
                    })
                })
                .collect();
            if matches.is_empty() {
                continue;
            }
            matches.sort_by_key(|found| found.line);
            results.push(search::FileMatch { path: path.clone(), matches });
        }
        if results.is_empty() {
            return;
        }
        let query = i18n::t!("diagnostics.title");
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.search_panel = Some(SearchState {
            query: query.clone(),
            case_sensitive: true,
            is_regex: false,
            results,
            results_query: Some(query),
            selected: 0,
            error: None,
            running: false,
            active_search: None,
            focus,
        });
        cx.notify();
    }

    // ── code actions（⌘.・M11） ──

    fn open_code_actions(&mut self, _: &CodeActions, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
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
        let debug = std::env::var_os("SHIRUSHI_LSP_DEBUG").is_some();
        let (Some((path, line, character, position)), true) = (info, self.lsp_initialized) else {
            if debug {
                eprintln!("code_actions: LSP 未初期化 or 対応外ファイル");
            }
            return;
        };
        let Some(lsp) = &self.lsp else {
            if debug {
                eprintln!("code_actions: LSP クライアント無し");
            }
            return;
        };
        // 同じ行の診断だけを context に渡す（quickfix の対象特定）。
        let diagnostics = self
            .raw_diagnostics
            .get(&path)
            .and_then(|value| value.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic
                            .pointer("/range/start/line")
                            .and_then(|v| v.as_u64())
                            .map(|l| l as u32 == line)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let receiver = lsp.code_actions(&path, line, character, serde_json::Value::Array(diagnostics));
        cx.spawn(async move |_workspace, cx| {
            let response = receiver.await;
            if std::env::var_os("SHIRUSHI_LSP_DEBUG").is_some() {
                eprintln!("code_actions 応答: {response:?}");
            }
            let Ok(Ok(value)) = response else {
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                let Some(array) = value.as_array() else {
                    return;
                };
                let items: Vec<(SharedString, serde_json::Value)> = array
                    .iter()
                    .filter_map(|action| {
                        let title = action.get("title")?.as_str()?.to_string();
                        Some((SharedString::from(title), action.clone()))
                    })
                    .take(20)
                    .collect();
                if items.is_empty() {
                    return;
                }
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                let position = workspace
                    .active_editor()
                    .and_then(|editor| editor.read(cx).caret_window_position())
                    .or(position)
                    .unwrap_or_else(|| point(px(220.), px(180.)));
                workspace.code_actions = Some(CodeActionsState { items, selected: 0, position, focus });
                cx.notify();
            });
        })
        .detach();
    }

    fn close_code_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_actions.take().is_none() {
            return;
        }
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// 選択中アクションを適用する。edit が無ければ codeAction/resolve で解決してから。
    fn confirm_code_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let action = self
            .code_actions
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .map(|(_, action)| action.clone());
        self.close_code_actions(window, cx);
        let Some(action) = action else {
            return;
        };
        if let Some(edit) = action.get("edit").filter(|edit| !edit.is_null()) {
            let edit = edit.clone();
            self.apply_workspace_edit(&edit, cx);
            return;
        }
        // edit が遅延解決（ra はこのパターンが多い）→ resolve してから適用。
        let Some(lsp) = &self.lsp else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let receiver = lsp.resolve_code_action(action);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(resolved)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, _window, cx| {
                if let Some(edit) = resolved.get("edit").filter(|edit| !edit.is_null()) {
                    let edit = edit.clone();
                    workspace.apply_workspace_edit(&edit, cx);
                } else {
                    eprintln!("code action: 適用できる edit が無い（command のみは v1 非対応）");
                }
            });
        })
        .detach();
    }

    fn on_code_actions_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_code_actions(window, cx),
            "up" => {
                if let Some(state) = self.code_actions.as_mut() {
                    let len = state.items.len();
                    state.selected = (state.selected + len - 1) % len.max(1);
                    cx.notify();
                }
            }
            "down" => {
                if let Some(state) = self.code_actions.as_mut() {
                    let len = state.items.len().max(1);
                    state.selected = (state.selected + 1) % len;
                    cx.notify();
                }
            }
            "enter" => self.confirm_code_action(window, cx),
            _ => self.close_code_actions(window, cx),
        }
    }

    /// ⌘. のポップアップ（補完と同じ見た目・キャレット直下）。
    fn render_code_actions(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.code_actions.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let focus = state.focus.clone();
        let selected = state.selected;
        let list = div().flex().flex_col().max_h(px(280.)).overflow_hidden().children(
            state.items.iter().enumerate().map(|(row, (title, _))| {
                let is_selected = row == selected;
                div()
                    .id(("code-action", row))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(is_selected, |element| element.bg(accent.alpha(0.16)))
                    .hover(|style| style.bg(theme.bg3))
                    .child(div().flex_none().text_size(px(10.)).text_color(accent).child("✦"))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .text_color(if is_selected { theme.fg0 } else { theme.fg1 })
                            .child(title.clone()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if let Some(state) = this.code_actions.as_mut() {
                                state.selected = row;
                            }
                            this.confirm_code_action(window, cx)
                        }),
                    )
            }),
        );
        Some(
            div()
                .absolute()
                .inset_0()
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_code_actions_key_down))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_code_actions(window, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .left(state.position.x)
                        .top(state.position.y + px(2.))
                        .w(px(380.))
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

    // ── 参照検索（⇧F12・M11） ──

    /// 参照を検索して ⌘⇧F の結果パネル（ファイル別グルーピング UI）で見せる。
    fn find_references(&mut self, _: &FindReferences, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
            return;
        };
        let info = {
            let view = editor.read(cx);
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(|path| {
                    let (line, character) = view.cursor_lsp_position();
                    let snapshot = view.buffer().snapshot();
                    let head = view.buffer().selections().first().map(|s| s.head).unwrap_or(0);
                    let word = snapshot
                        .word_range_at(head)
                        .map(|range| view.buffer().text_range(range))
                        .unwrap_or_default();
                    (path.to_path_buf(), line, character, word)
                })
        };
        let (Some((path, line, character, word)), true) = (info, self.lsp_initialized) else {
            return;
        };
        let Some(lsp) = &self.lsp else {
            return;
        };
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let receiver = lsp.references(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            // Location[] → ファイル別に集約し、プレビュー行のために背景でファイルを読む。
            let results = cx
                .background_executor()
                .spawn(async move { locations_to_file_matches(host.as_ref(), &value) })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                let query = i18n::t!("searchpanel.reference_query", "word" => word);
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                workspace.search_panel = Some(SearchState {
                    query: query.clone(),
                    case_sensitive: true,
                    is_regex: false,
                    results,
                    results_query: Some(query),
                    selected: 0,
                    error: None,
                    running: false,
                    active_search: None,
                    focus,
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// hover をキーで出す（⌘K ⌘I）。キャレット位置で要求し、キャレット直上に出す。
    fn show_hover_at_caret(&mut self, _: &ShowHover, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let (line, character, anchor) = {
            let view = editor.read(cx);
            let (line, character) = view.cursor_lsp_position();
            (line, character, view.caret_window_position())
        };
        let Some(anchor) = anchor else {
            return;
        };
        self.request_hover(line, character, anchor, window, cx);
    }

    /// エディタの hover dwell（[`EditorHoverEvent::Dwell`]）→ LSP hover 要求（M10）。
    fn on_editor_hover(
        &mut self,
        editor: &Entity<EditorView>,
        event: &EditorHoverEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (line, character, anchor) = match *event {
            EditorHoverEvent::Dwell { line, character, anchor } => (line, character, anchor),
            EditorHoverEvent::Cancel => {
                self.close_hover(cx);
                return;
            }
        };
        if self.active_editor().as_ref() != Some(editor) {
            return;
        }
        // 補完ポップアップが開いている間は hover を出さない（重なると鬱陶しい）。
        if self.completion.is_some() {
            return;
        }
        self.request_hover(line, character, anchor, window, cx);
    }

    /// LSP へ hover を要求し、応答があればポップアップを出す（dwell / ⌘K ⌘I 共通）。
    fn request_hover(
        &mut self,
        line: u32,
        character: u32,
        anchor: Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
            return;
        };
        let path = {
            let view = editor.read(cx);
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(Path::to_path_buf)
        };
        let Some(path) = path else {
            return;
        };
        if !self.lsp_initialized {
            return;
        }
        let Some(lsp) = &self.lsp else {
            return;
        };
        self.hover_generation = self.hover_generation.wrapping_add(1);
        let generation = self.hover_generation;
        let receiver = lsp.hover(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, _window, cx| {
                if workspace.hover_generation != generation {
                    return;
                }
                let lines = parse_hover_lines(&value);
                if lines.is_empty() {
                    return;
                }
                const MAX_HOVER_LINES: usize = 24;
                let mut lines: Vec<SharedString> = lines
                    .into_iter()
                    .take(MAX_HOVER_LINES + 1)
                    .map(SharedString::from)
                    .collect();
                if lines.len() > MAX_HOVER_LINES {
                    lines.truncate(MAX_HOVER_LINES);
                    lines.push(SharedString::from("…"));
                }
                workspace.hover = Some(HoverState { lines, position: anchor });
                cx.notify();
            });
        })
        .detach();
    }

    // ── ファイル監視（watch 基盤・M10） ──

    /// アクティブプロジェクトの worktree 監視を開始する（既存の監視は破棄して張り替え）。
    /// local のみ（remote の watch subscription は M13）。イベントは 200ms 合流してから反映する。
    fn start_watcher(&mut self, cx: &mut Context<Self>) {
        self._watch = None;
        self._watch_pump = None;
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        if worktree.host().is_remote() {
            return;
        }
        let root = worktree.root().to_path_buf();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
        let debug = std::env::var_os("SHIRUSHI_WATCH_DEBUG").is_some();
        match project::watch_root(&root, move |paths| {
            if debug {
                eprintln!("watch: raw event {paths:?}");
            }
            let _ = sender.unbounded_send(paths);
        }) {
            Ok(watch) => {
                if debug {
                    eprintln!("watch: 監視開始 {}", root.display());
                }
                self._watch = Some(watch);
            }
            Err(error) => {
                eprintln!("{error:#}");
                return;
            }
        }
        self._watch_pump = Some(cx.spawn(async move |workspace, cx| {
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
                let updated = workspace
                    .update(cx, |workspace, cx| workspace.handle_watch_events(paths, cx));
                if debug {
                    eprintln!("watch: update {:?}", updated.as_ref().map(|_| "ok").map_err(|e| format!("{e}")));
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
    fn handle_watch_events(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let mut tree_changed = false;
        let mut git_changed = false;
        for path in &paths {
            // .git 配下は index/HEAD/refs だけ git 更新の合図に使う（objects 等の湧きは無視）。
            if path.components().any(|component| component.as_os_str() == ".git") {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                let in_refs = path.components().any(|component| component.as_os_str() == "refs");
                if in_refs || matches!(name, "index" | "HEAD" | "ORIG_HEAD" | "packed-refs") {
                    git_changed = true;
                }
                continue;
            }
            // 開いているバッファへ配送（gitignore に関係なく）。
            if let Some(tab) = self.tabs.iter().find(|tab| &tab.path == path) {
                let editor = tab.editor.clone();
                editor.update(cx, |view, cx| view.handle_external_change(cx));
                git_changed = true;
                continue;
            }
            if !worktree.is_ignored(path) {
                tree_changed = true;
                git_changed = true;
            }
        }
        if tree_changed {
            if let Some(slot) = self.projects.get_mut(self.active) {
                slot.refresh();
            }
        }
        if git_changed {
            self.refresh_git_status(cx);
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |view, cx| view.refresh_diff(cx));
            }
        }
        // Todo ボード: どの書き手（AI/CLI/手編集）が todos.md を変えても板が追従する（M12-10 の心臓部）。
        if paths
            .iter()
            .any(|path| path.ends_with(std::path::Path::new(".shirushi/todos.md")))
        {
            self.reload_todo_board(cx);
        }
        if tree_changed || git_changed {
            cx.notify();
        }
    }

    /// settings の実効値（font_size/tab_size/soft_wrap）を全エディタへ配る（live 反映・M10-13）。
    fn apply_editor_settings(&mut self, cx: &mut Context<Self>) {
        let current = settings::get(cx);
        let soft_wrap = current.soft_wrap || std::env::var_os("SHIRUSHI_SOFT_WRAP").is_some();
        let (font_size, tab_size) = (current.font_size, current.tab_size);
        let editors: Vec<Entity<EditorView>> = self
            .tabs
            .iter()
            .map(|tab| tab.editor.clone())
            .chain(self.split_editor.clone())
            .collect();
        for editor in editors {
            editor.update(cx, |view, cx| {
                view.set_typography(font_size, tab_size, cx);
                view.set_soft_wrap(soft_wrap, cx);
            });
        }
    }

    // ── プロジェクト色ピッカー（レール右クリック → .shirushi/settings.json へ・M12-11） ──

    /// 選んだ色をプロジェクトへ適用し `.shirushi/settings.json` に保存（再起動後も効く）。
    fn apply_project_color(
        &mut self,
        project_index: usize,
        color: Hsla,
        hex: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_color_picker(window, cx);
        let Some(slot) = self.projects.get_mut(project_index) else {
            return;
        };
        slot.color = color;
        let remote_key = slot
            .worktree
            .host()
            .is_remote()
            .then(|| slot.worktree.host().display_name().to_string());
        match &remote_key {
            // ローカルは従来どおり `.shirushi/settings.json` へ（リポジトリ共有・チーム寄り）。
            None => {
                let settings_path = slot.worktree.root().join(".shirushi/settings.json");
                if let Err(error) = settings_core::persist_user_value(
                    &settings_path,
                    "color",
                    serde_json::Value::String(hex.to_string()),
                ) {
                    eprintln!(".shirushi への色の保存に失敗: {error:#}");
                }
            }
            // リモートは `.shirushi` がリモート側にあり使えないので、手動色をローカル DB に焼く（M13 #3b・再接続で復元）。
            Some(key) => {
                if let (Some(storage), Ok(value)) = (
                    self.storage.clone(),
                    u32::from_str_radix(hex.trim_start_matches('#'), 16),
                ) {
                    let _ = storage.set_host_color(key, value);
                }
            }
        }
        // アクティブなら全ペイン（タブ + 分割）のキャレット等アクセントへ波及。
        // レール/タブは render 時に slot.color を読むので notify で追従する（明示波及が要るのはキャレットだけ）。
        if project_index == self.active {
            let editors: Vec<Entity<EditorView>> = self
                .tabs
                .iter()
                .map(|tab| tab.editor.clone())
                .chain(self.split_editor.clone())
                .collect();
            for editor in editors {
                editor.update(cx, |view, cx| view.set_accent(color, cx));
            }
        }
        cx.notify();
    }

    /// 色ピッカーを開く（右クリック位置 or キーボード起動時のアンカー位置）。hex 入力へフォーカス。
    fn open_color_picker(
        &mut self,
        project_index: usize,
        position: Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.color_picker = Some(ColorPickerState { project_index, position, hex: String::new(), focus });
        cx.notify();
    }

    /// 色ピッカーを閉じ、フォーカスをアクティブエディタへ戻す（rename と同型）。
    fn close_color_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.color_picker.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    /// ⌘K⌘C / コマンドパレットからアクティブプロジェクトの色ピッカーを開く。
    /// マウス位置が無いので、アクティブなレール項目の位置（pt_2 8px + index*38）にアンカーする。
    fn open_project_color(&mut self, _: &ProjectColor, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = gpui::point(px(RAIL_WIDTH), px(8. + self.active as f32 * 38. + 4.));
        self.open_color_picker(self.active, anchor, window, cx);
    }

    /// 色ピッカーの hex 入力キー処理（rename と同型 + 16 進フィルタ）。
    fn on_color_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_color_picker(window, cx),
            "enter" => {
                let Some(state) = self.color_picker.as_ref() else {
                    return;
                };
                let project_index = state.project_index;
                let candidate = format!("#{}", state.hex);
                // 6 桁揃ったときだけ適用（parse_hex_color が len==6 を要求）。未完・不正は無視。
                if let Some(color) = parse_hex_color(&candidate) {
                    self.apply_project_color(project_index, color, &candidate, window, cx);
                }
            }
            "backspace" => {
                if let Some(state) = self.color_picker.as_mut() {
                    state.hex.pop();
                    cx.notify();
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if let Some(state) = self.color_picker.as_mut() {
                    // 16 進 6 桁まで（`#` はラベルで出すので取り込まない）。
                    for ch in text.chars().filter(|ch| ch.is_ascii_hexdigit()) {
                        if state.hex.len() >= 6 {
                            break;
                        }
                        state.hex.push(ch);
                    }
                    cx.notify();
                }
            }
        }
    }

    /// 色ピッカー（識別用の厳選スウォッチ + 任意 hex 入力・M12-11 / Peacock 拡張）。
    fn render_color_picker(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.color_picker.as_ref()?;
        let project_index = state.project_index;
        let position = state.position;
        let theme = self.theme.clone();
        // 対象プロジェクトの現在色を hex 入力のキャレット色に使う（文脈のヒント）。
        let anchor_color = self
            .projects
            .get(project_index)
            .map(|slot| slot.color)
            .unwrap_or_else(|| self.accent());
        let hex_display: SharedString = SharedString::from(state.hex.clone());
        let hex_empty = state.hex.is_empty();
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_color_picker(window, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .left(position.x + px(8.))
                        .top(position.y)
                        .w(px(168.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .p(px(8.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
                        ])
                        .track_focus(&state.focus)
                        .on_key_down(cx.listener(Self::on_color_key_down))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div().flex().flex_wrap().gap(px(6.)).children(
                                theme_core::IDENTITY_PALETTE_HEXES.iter().enumerate().map(
                                    |(swatch_index, &value)| {
                                        let hex = format!("#{value:06x}");
                                        let color = parse_hex_color(&hex)
                                            .unwrap_or_else(|| project_color(swatch_index));
                                        div()
                                            .id(("color-swatch", swatch_index))
                                            .size(px(22.))
                                            .rounded(px(6.))
                                            .bg(color)
                                            .cursor_pointer()
                                            .hover(|style| style.border_2().border_color(gpui::white()))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.apply_project_color(project_index, color, &hex, window, cx)
                                                }),
                                            )
                                    },
                                ),
                            ),
                        )
                        .child(
                            // 任意 hex 入力行（許可リスト外の色も許すエスケープハッチ・UI-SPEC §1.2）。
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .h(px(24.))
                                .px(px(6.))
                                .bg(theme.bg1)
                                .border_1()
                                .border_color(theme.border)
                                .rounded(px(6.))
                                .text_size(px(12.))
                                .text_color(theme.fg0)
                                .child(div().flex_none().text_color(theme.fg2).child(SharedString::from("#")))
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(hex_empty, |element| element.text_color(theme.fg2))
                                        .child(if hex_empty {
                                            SharedString::from(i18n::t!("color.hex_placeholder"))
                                        } else {
                                            hex_display
                                        }),
                                )
                                .child(div().flex_none().w(px(1.5)).h(px(12.)).bg(anchor_color)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// レール項目の右クリックメニュー（M10-2）。色スウォッチ + 新規窓 / レールから外す /
    /// （worktree タブなら）worktree・ブランチ削除。破壊的操作は二段確認。
    fn render_rail_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.rail_menu.as_ref()?;
        let index = menu.project_index;
        let position = menu.position;
        let confirm = menu.confirm;
        let theme = self.theme.clone();
        let slot = self.projects.get(index)?;
        let is_worktree = slot.worktree_branch.is_some();
        let slot_root = slot.worktree.root().to_path_buf();
        let (bg2, bg3, border, fg0, fg1, fg2, err) =
            (theme.bg2, theme.bg3, theme.border, theme.fg0, theme.fg1, theme.fg2, theme.err);

        // 通常のメニュー行（アイコン + ラベル）。danger=true は削除系（hover で赤・armed で確認文言）。
        let make_row = move |row_id: &'static str,
                             icon: &'static str,
                             label: SharedString,
                             danger: bool,
                             armed: bool| {
            let base = if armed { err } else { fg1 };
            div()
                .id(row_id)
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(base)
                .cursor_pointer()
                .hover(move |style| {
                    if danger {
                        style.bg(bg3).text_color(err)
                    } else {
                        style.bg(bg3).text_color(fg0)
                    }
                })
                .child(div().w(px(14.)).flex_none().text_color(if armed { err } else { fg2 }).child(icon))
                .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(label))
        };

        let mut menu_box = div()
            .absolute()
            .left(position.x + px(8.))
            .top(position.y)
            .w(px(228.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(4.))
            .bg(bg2)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
            ])
            // メニュー内クリックは背後の backdrop へ伝えない（閉じ・誤爆防止）。
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // 色スウォッチ列（速い色変更。許可リスト外の hex は「その他の色…」へ）。
            .child(
                div().flex().flex_wrap().gap(px(5.)).p(px(4.)).children(
                    theme_core::IDENTITY_PALETTE_HEXES.iter().enumerate().map(|(swatch_index, &value)| {
                        let hex = format!("#{value:06x}");
                        let color = parse_hex_color(&hex).unwrap_or_else(|| project_color(swatch_index));
                        div()
                            .id(("rail-swatch", swatch_index))
                            .size(px(20.))
                            .rounded(px(6.))
                            .bg(color)
                            .cursor_pointer()
                            .hover(|style| style.border_2().border_color(gpui::white()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.apply_project_color(index, color, &hex, window, cx);
                                    this.close_rail_menu(cx);
                                }),
                            )
                    }),
                ),
            )
            // 「その他の色…」= hex 入力つきフル色ピッカーへ（エスケープハッチ）。
            .child(
                make_row("rail-more-colors", "🎨", SharedString::from(i18n::t!("rail.menu_more_colors")), false, false)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.close_rail_menu(cx);
                            this.open_color_picker(index, position, window, cx);
                        }),
                    ),
            )
            .child(div().h(px(1.)).bg(border).my(px(2.)))
            // 新しいウィンドウで開く（旧・既定挙動を明示操作に格下げ）。
            .child({
                let path = slot_root.clone();
                make_row("rail-new-window", "⧉", SharedString::from(i18n::t!("rail.menu_new_window")), false, false)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.close_rail_menu(cx);
                            this.open_folder_as_window(path.clone(), cx);
                        }),
                    )
            })
            .child(div().h(px(1.)).bg(border).my(px(2.)))
            // レールから外す（安全＝ディスク無傷。表示だけ消す）。
            .child(
                make_row("rail-remove", "✕", SharedString::from(i18n::t!("rail.menu_remove")), false, false)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.remove_project_slot(index, window, cx)),
                    ),
            );

        // worktree タブだけ: worktree 削除 / worktree ごとブランチ削除（二段確認）。
        if is_worktree {
            let wt_armed = confirm == Some(RailMenuAction::RemoveWorktree);
            let wt_label = if wt_armed {
                i18n::t!("rail.menu_confirm_delete")
            } else {
                i18n::t!("rail.menu_remove_worktree")
            };
            menu_box = menu_box.child(
                make_row("rail-remove-worktree", "🗂", SharedString::from(wt_label), true, wt_armed)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if wt_armed {
                                this.remove_slot_worktree(index, window, cx);
                            } else {
                                this.arm_rail_confirm(RailMenuAction::RemoveWorktree, cx);
                            }
                        }),
                    ),
            );
            let br_armed = confirm == Some(RailMenuAction::DeleteBranch);
            let br_label = if br_armed {
                i18n::t!("rail.menu_confirm_delete")
            } else {
                i18n::t!("rail.menu_delete_branch")
            };
            menu_box = menu_box.child(
                make_row("rail-delete-branch", "🗑", SharedString::from(br_label), true, br_armed)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if br_armed {
                                this.delete_slot_branch(index, window, cx);
                            } else {
                                this.arm_rail_confirm(RailMenuAction::DeleteBranch, cx);
                            }
                        }),
                    ),
            );
        }

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| this.close_rail_menu(cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| this.close_rail_menu(cx)))
                .child(menu_box)
                .into_any_element(),
        )
    }

    /// レールメニューの破壊的操作を二段確認の「1 段目」にする（もう一度クリックで実行）。
    fn arm_rail_confirm(&mut self, action: RailMenuAction, cx: &mut Context<Self>) {
        if let Some(menu) = self.rail_menu.as_mut() {
            menu.confirm = Some(action);
            cx.notify();
        }
    }

    // ── Agent パネル連携（トースト・色リンク・生中継・M12-3/4/5） ──

    fn on_panel_event(&mut self, _panel: Entity<AgentPanel>, event: &agent_panel::PanelEvent, cx: &mut Context<Self>) {
        match event {
            agent_panel::PanelEvent::OpenHistoryRequest => {
                // window が無いので次の render で消化する（pending_transient_tab と同じ迂回・#5）。
                self.pending_open_history = true;
                cx.notify();
            }
            agent_panel::PanelEvent::TurnEnded { thread, color, summary } => {
                self.waiting_thread = None;
                self.push_toast(SharedString::from(format!("● {thread} — {summary}")), *color, cx);
                // Todo ボード: そのスレッドに送った項目の pulse を解除し、板を読み直す
                // （エージェントが todos.md をチェックしたら watch より先に即反映・M12-10）。
                if let Some(board) = self.todo_board.as_mut() {
                    board.running.retain(|_, running_color| running_color != color);
                    self.reload_todo_board(cx);
                }
            }
            agent_panel::PanelEvent::PermissionWaiting { thread, color } => {
                self.waiting_thread = Some((thread.clone(), *color));
                self.push_toast(
                    SharedString::from(format!("● {thread} — {}", i18n::t!("agent.waiting_permission"))),
                    *color,
                    cx,
                );
            }
            agent_panel::PanelEvent::OpenDiffRequest { title, old_text, new_text } => {
                // 提案 diff を transient タブでレビュー（M12-6）。window が要るのでイベントから取得不可 →
                // 承認カードはアクティブ窓でしか押せないため、直近 focus の window handle を使う。
                if let Some(diff_text) = project::unified_diff_texts(old_text, new_text, title) {
                    let mut buffer = Buffer::from_str(&diff_text);
                    buffer.set_read_only(true);
                    self.pending_transient_tab =
                        Some((PathBuf::from(i18n::t!("difftab.proposal_title", "title" => title)), buffer));
                    cx.notify();
                }
            }
            agent_panel::PanelEvent::FilesTouched { files, color } => {
                for file in files {
                    self.agent_touched.insert(file.clone(), *color);
                    // 開いていれば gutter をスレッド色に（生中継の帰属・M12-3）。
                    if let Some(tab) = self.tabs.iter().find(|tab| &tab.path == file) {
                        let editor = tab.editor.clone();
                        let color = *color;
                        editor.update(cx, |view, cx| view.set_agent_mark_color(Some(color), cx));
                    }
                }
                cx.notify();
            }
        }
    }

    /// トーストを積む（右下・5 秒で自動で消える・UI-SPEC §8）。
    fn push_toast(&mut self, text: SharedString, color: Hsla, cx: &mut Context<Self>) {
        self.toast_gen = self.toast_gen.wrapping_add(1);
        let generation = self.toast_gen;
        self.toasts.push((text, color, generation));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.toasts.retain(|(_, _, gen)| *gen != generation);
                cx.notify();
            });
        })
        .detach();
    }

    /// トースト描画（右下スタック・M12-5）。
    fn render_toasts(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.toasts.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        Some(
            div()
                .absolute()
                .bottom(px(38.))
                .right(px(16.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(self.toasts.iter().map(|(text, color, generation)| {
                    div()
                        .id(("toast", *generation as usize))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(8.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(color.alpha(0.5))
                        .rounded(px(8.))
                        .shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
                        ])
                        .text_size(px(12.))
                        .text_color(theme.fg0)
                        .child(text.clone())
                }))
                .into_any_element(),
        )
    }

    // ── hot exit（クラッシュ耐性・M10。置き場 = Turso storage crate） ──

    /// dirty バッファのスナップショットを（2 秒デバウンスで）DB へ書く。クリーンになった分は消す。
    /// 編集の notify 毎に呼ばれるが、世代番号で最後の 1 回だけ実行される。書き込みは背景。
    fn schedule_hot_exit_snapshot(&mut self, cx: &mut Context<Self>) {
        let debug = std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some();
        let Some(storage) = self.storage.clone() else {
            if debug {
                eprintln!("hotexit: storage=None でスキップ");
            }
            return;
        };
        self.hot_exit_gen = self.hot_exit_gen.wrapping_add(1);
        let generation = self.hot_exit_gen;
        if debug {
            eprintln!("hotexit: 予約 gen={generation}");
        }
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let snapshot: Vec<(PathBuf, String, bool)> = match workspace.update(cx, |workspace, cx| {
                if workspace.hot_exit_gen != generation {
                    return Vec::new(); // 後続の編集が来ている＝その回に任せる
                }
                if workspace.hot_exit_pending.is_some() {
                    // 復元/破棄が未決の間は書きも消しもしない（クリーン扱いで候補行を消してしまう）。
                    return Vec::new();
                }
                workspace
                    .tabs
                    .iter()
                    .map(|tab| {
                        let view = tab.editor.read(cx);
                        let dirty = view.buffer().is_dirty();
                        let text = if dirty { view.buffer().text() } else { String::new() };
                        (tab.path.clone(), text, dirty)
                    })
                    .collect()
            }) {
                Ok(snapshot) => snapshot,
                Err(_) => return,
            };
            if std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some() {
                eprintln!("hotexit: tick gen={generation} snapshot={} 件", snapshot.len());
            }
            if snapshot.is_empty() {
                return;
            }
            cx.background_executor()
                .spawn(async move {
                    for (path, text, dirty) in snapshot {
                        let result = if dirty {
                            storage.save_hot_exit(&path, &text)
                        } else {
                            storage.remove_hot_exit(&path)
                        };
                        if let Err(error) = result {
                            eprintln!("hot exit スナップショットに失敗: {error:#}");
                        }
                    }
                })
                .await;
        })
        .detach();
    }

    /// 起動時に前回の未保存スナップショットを探し、あれば復元/破棄バーを出す（main から呼ぶ）。
    pub fn check_hot_exit_restore(&mut self, cx: &mut Context<Self>) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        cx.spawn(async move |workspace, cx| {
            let rows = cx
                .background_executor()
                .spawn(async move { storage.load_hot_exit_all() })
                .await;
            let Ok(rows) = rows else { return };
            if rows.is_empty() {
                return;
            }
            if std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some() {
                eprintln!("hotexit: 復元候補 {} 件（バー表示）", rows.len());
            }
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.hot_exit_pending = Some(rows);
                cx.notify();
            });
        })
        .detach();
    }

    /// 復元バーの「復元」: スナップショットを各バッファへ流し込む（開いていなければ開く）。
    /// 置換は 1 Transaction なので undo で復元前に戻れる。復元後は dirty ＝次の tick で再スナップショット。
    fn restore_hot_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rows) = self.hot_exit_pending.take() else {
            return;
        };
        for (path, content) in rows {
            if !self.tabs.iter().any(|tab| tab.path == path) {
                self.open_file_sync(path.clone(), window, cx);
            }
            if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path) {
                let editor = tab.editor.clone();
                editor.update(cx, |view, cx| view.replace_all_text(&content, cx));
            } else {
                eprintln!("hot exit: 復元先を開けない（スキップ）: {}", path.display());
            }
        }
        cx.notify();
    }

    /// 復元バーの「破棄」: スナップショットを消してバーを閉じる。
    fn discard_hot_exit(&mut self, cx: &mut Context<Self>) {
        self.hot_exit_pending = None;
        if let Some(storage) = self.storage.clone() {
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.clear_hot_exit() {
                        eprintln!("hot exit の破棄に失敗: {error:#}");
                    }
                })
                .detach();
        }
        cx.notify();
    }

    /// 正常終了時の後始末（Quit アクションから）。スナップショットは破棄する（仕様: 正常終了で破棄）。
    pub fn prepare_quit(&mut self) {
        if let Some(storage) = &self.storage {
            if let Err(error) = storage.clear_hot_exit() {
                eprintln!("hot exit のクリアに失敗: {error:#}");
            }
        }
    }

    // ── ⌃G 行ジャンプ（M10-12） ──

    fn open_goto_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_editor().is_none() {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.goto_line = Some((String::new(), focus));
        cx.notify();
    }

    fn close_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.goto_line.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    fn on_goto_line_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_goto_line(window, cx),
            "enter" => {
                let line = self
                    .goto_line
                    .as_ref()
                    .and_then(|(value, _)| value.trim().parse::<usize>().ok());
                self.close_goto_line(window, cx);
                if let (Some(line), Some(editor)) = (line, self.active_editor()) {
                    self.record_nav_position(cx); // 行ジャンプもナビ履歴へ
                    editor.update(cx, |view, cx| {
                        view.reveal_position(line.saturating_sub(1), 0, cx)
                    });
                }
            }
            "backspace" => {
                if let Some((value, _)) = self.goto_line.as_mut() {
                    value.pop();
                    cx.notify();
                }
            }
            _ => {
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.chars().all(|c| c.is_ascii_digit()) && !text.is_empty() {
                    if let Some((value, _)) = self.goto_line.as_mut() {
                        if value.len() < 9 {
                            value.push_str(text);
                            cx.notify();
                        }
                    }
                }
            }
        }
    }

    /// ⌃G のミニ入力（中央上の小さな箱）。
    fn render_goto_line(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (value, focus) = self.goto_line.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let display: SharedString = if value.is_empty() {
            SharedString::from(i18n::t!("goto.placeholder"))
        } else {
            SharedString::from(value.clone())
        };
        let color = if value.is_empty() { theme.fg2 } else { theme.fg0 };
        Some(
            div()
                .absolute()
                .top(px(96.))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(220.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(30.))
                        .px(px(10.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(accent)
                        .rounded(px(8.))
                        .shadow(vec![gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4))
                            .blur_radius(px(16.))])
                        .track_focus(focus)
                        .on_key_down(cx.listener(Self::on_goto_line_key_down))
                        .text_size(px(12.5))
                        .text_color(color)
                        .child(div().flex_none().text_color(theme.fg2).child(":"))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(display))
                        .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent)),
                )
                .into_any_element(),
        )
    }

    // ── ナビゲーション履歴（M10-11: ⌃- 戻る / ⌃⇧- 進む） ──

    /// 現在位置（アクティブタブのパス + キャレット byte offset）。
    fn current_nav_position(&self, cx: &App) -> Option<(PathBuf, usize)> {
        let editor = self.active_editor()?;
        let path = self.active_tab_path()?;
        let offset = editor.read(cx).buffer().selections().first().map(|s| s.head).unwrap_or(0);
        Some((path, offset))
    }

    /// ジャンプ級の移動の**直前**に呼ぶ: 現在位置を「戻る」へ積み、「進む」を捨てる。
    fn record_nav_position(&mut self, cx: &App) {
        let Some(position) = self.current_nav_position(cx) else {
            return;
        };
        if self.nav_back.last() == Some(&position) {
            return; // 連続同位置は積まない
        }
        self.nav_back.push(position);
        if self.nav_back.len() > 100 {
            self.nav_back.remove(0);
        }
        self.nav_forward.clear();
    }

    fn navigate_back(&mut self, _: &NavigateBack, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav_back.pop() else {
            return;
        };
        if let Some(current) = self.current_nav_position(cx) {
            self.nav_forward.push(current);
        }
        self.navigate_to(target, window, cx);
    }

    fn navigate_forward(&mut self, _: &NavigateForward, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav_forward.pop() else {
            return;
        };
        if let Some(current) = self.current_nav_position(cx) {
            self.nav_back.push(current);
        }
        self.navigate_to(target, window, cx);
    }

    /// 履歴の 1 点へ移動する（閉じたファイルは開き直す）。
    fn navigate_to(&mut self, target: (PathBuf, usize), window: &mut Window, cx: &mut Context<Self>) {
        let (path, offset) = target;
        if self.active_tab_path().as_ref() == Some(&path) {
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |view, cx| view.select_byte_range(offset..offset, cx));
            }
            return;
        }
        // 別ファイル: 開いて（開いていれば切替のみ）から着地する。open_file は背景読みなので
        // 完了後に reveal できるよう自前で合流する。
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |view, cx| view.select_byte_range(offset..offset, cx));
            }
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let host = worktree.host().clone();
        let read_path = path.clone();
        cx.spawn(async move |_workspace, cx| {
            let content = cx
                .background_executor()
                .spawn(async move { host.read_file(&read_path) })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| match content {
                Ok(content) => {
                    workspace.open_loaded_file(path, content, window, cx);
                    if let Some(editor) = workspace.active_editor() {
                        editor.update(cx, |view, cx| view.select_byte_range(offset..offset, cx));
                    }
                }
                Err(error) => eprintln!("履歴のファイルを開けない: {error:#}"),
            });
        })
        .detach();
    }

    /// hover ポップアップを閉じる（タイプ・クリック・タブ切替・新しい dwell で呼ぶ）。
    fn close_hover(&mut self, cx: &mut Context<Self>) {
        if self.hover.take().is_some() {
            // 進行中の応答も無効化する。
            self.hover_generation = self.hover_generation.wrapping_add(1);
            cx.notify();
        }
    }

    /// エディタの確定入力（[`EditorInputEvent::Typed`]）→ 補完の自動トリガ（M10）。
    /// 識別子文字 = 開いていれば絞り込み・閉じていれば新語でポップアップ。`.`/`::` = 新規要求。
    /// その他 = 閉じる。Esc で閉じた語は語頭が変わるまで再表示しない。
    fn on_editor_typed(
        &mut self,
        editor: &Entity<EditorView>,
        event: &EditorInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match event {
            EditorInputEvent::Typed(text) => text,
            EditorInputEvent::HunkClicked { hunk, position } => {
                if self.active_editor().as_ref() == Some(editor) {
                    self.on_hunk_clicked(hunk.clone(), *position, cx);
                }
                return;
            }
            EditorInputEvent::CaretJumped { from } => {
                // 大距離クリック → 移動前の位置をナビ履歴へ（M10-11）。
                if self.active_editor().as_ref() == Some(editor) {
                    if let Some(path) = self.active_tab_path() {
                        let position = (path, *from);
                        if self.nav_back.last() != Some(&position) {
                            self.nav_back.push(position);
                            if self.nav_back.len() > 100 {
                                self.nav_back.remove(0);
                            }
                            self.nav_forward.clear();
                        }
                    }
                }
                return;
            }
        };
        // アクティブタブ以外（比較ビュー等）では何もしない。
        if self.active_editor().as_ref() != Some(editor) {
            return;
        }
        // タイプしたら hover は消す。
        self.close_hover(cx);
        let (before, word_start) = {
            let view = editor.read(cx);
            (view.text_before_caret(4), view.identifier_prefix_at_caret().0)
        };
        match classify_completion_trigger(text, &before) {
            CompletionTrigger::Identifier => {
                if let Some(state) = self.completion.as_mut() {
                    // 開いている → クライアント側絞り込み（LSP 再要求なし）。
                    if let Some(last) = text.chars().last() {
                        state.prefix.push(last);
                        state.selected = 0;
                        if state.filtered().is_empty() {
                            self.close_completion(window, cx);
                        }
                    }
                    cx.notify();
                    return;
                }
                // 閉じている → Esc した語でなければ自動ポップアップ。
                if self.completion_suppressed_word == Some(word_start) {
                    return;
                }
                self.completion_suppressed_word = None;
                self.request_completion(window, cx);
            }
            CompletionTrigger::Fresh => {
                // `.` / `::` はメンバ/パス補完を新規要求（Esc 抑止も解除）。
                self.completion_suppressed_word = None;
                if self.completion.is_some() {
                    self.close_completion(window, cx);
                }
                self.request_completion(window, cx);
            }
            CompletionTrigger::None => {
                if self.completion.is_some() {
                    self.close_completion(window, cx);
                }
            }
        }
    }

    fn switch_project(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.projects.len() || index == self.active {
            return;
        }
        self.active = index;
        self.load_active_slot(window, cx);
        self.save_state();
        cx.notify();
    }

    /// 現在の `self.active` スロットに合わせてビュー（LSP/端末/タブ/git/監視）を張り替える。
    /// プロジェクト切替（switch_project）と、アクティブスロットをレールから外した後（remove_project_slot）で共有。
    fn load_active_slot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // process/PTY は project の host と不可分。旧 host の session を次の project へ持ち越さない。
        let terminal_launch = Self::terminal_launch_for(self.active_slot());
        self.terminal_dock
            .update(cx, |dock, cx| dock.reset_launch(terminal_launch, cx));
        self.lsp = None;
        self._lsp_pump = None;
        self.lsp_root = None;
        self.lsp_language = None;
        self.lsp_initialized = false;
        self.lsp_sent_versions.clear();
        self.diagnostics.clear();
        // 分割ペインは旧プロジェクトのファイルを指しているので畳む。
        self.split_editor = None;
        // 切替先プロジェクトのタブ列を復元（無ければタブ無し＝空状態）。
        self.open_slot_files(window, cx);
        self.refresh_git_status(cx);
        self.update_agent_destination(cx);
        self.start_watcher(cx); // 監視対象を新しい worktree に張り替える
        if self.show_bottom {
            self.terminal_dock.update(cx, |dock, cx| {
                dock.ensure_active(cx);
            });
        }
    }

    /// Agent パネルの宛先チップにアクティブプロジェクト名・ブランチを反映する。
    fn update_agent_destination(&self, cx: &mut Context<Self>) {
        let (name, branch, host, cwd, files) = match self.active_slot() {
            Some(slot) => {
                // Add context の候補（プロジェクト先頭 60 ファイルの相対パス）。
                let files = slot
                    .worktree
                    .all_files(2000) // ＋context の fuzzy 絞り込み対象（M12-7 で 60→2000）
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
        // 最後に触った面が Agent なら AI スレッドタブを、そうでなければエディタタブを閉じる。
        // gpui は no-context バインドを最深で解決する（keymap では分離不能）ので、ここで振り分ける。
        // フォーカス依存だと transcript クリック等で判定を外すため、クリックで確定する agent_active を使う。
        if self.show_right && self.agent_active {
            self.agent_panel.update(cx, |panel, cx| panel.close_active_thread(cx));
            return;
        }
        self.close_active_editor(window, cx);
    }

    /// 次のエディタタブへ（⌘} = ⌘⇧]。末尾で先頭へ回る）。
    fn select_next_tab(&mut self, _: &SelectNextTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() > 1 {
            self.select_tab((self.active_tab + 1) % self.tabs.len(), window, cx);
        }
    }

    /// 前のエディタタブへ（⌘{ = ⌘⇧[。先頭で末尾へ回る）。
    fn select_prev_tab(&mut self, _: &SelectPrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if count > 1 {
            self.select_tab((self.active_tab + count - 1) % count, window, cx);
        }
    }

    /// 直近に閉じたタブを復元する（Chrome の ⌘⇧T）。⌘W と同じく最後に触った面で振り分ける。
    fn restore_closed_tab(&mut self, _: &RestoreClosedTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_right && self.agent_active {
            self.agent_panel.update(cx, |panel, cx| panel.restore_closed_thread(cx));
            return;
        }
        if let Some(path) = self.recently_closed_files.pop() {
            self.open_file(path, window, cx);
        }
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
            // Agent 側を触った → ⌘W の宛先を Agent スレッドに（クリックで確定）。
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, _cx| this.agent_active = true))
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

    /// project の Host 設定を TerminalDock が扱える launch spec に解決する。
    fn terminal_launch_for(slot: Option<&ProjectSlot>) -> TerminalLaunch {
        let (cwd, shell) = slot
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
        // 開発用: SHIRUSHI_TERM_ECHO="text" で起動時に text を表示してから shell へ
        // （file:line リンクの下線描画をオフスクリーン検証するためのフック・M13）。
        let shell = match std::env::var("SHIRUSHI_TERM_ECHO") {
            Ok(text) if !text.is_empty() && shell.is_none() => Some((
                "/bin/sh".to_string(),
                vec!["-c".to_string(), format!("echo '{text}'; exec zsh -f")],
            )),
            _ => shell,
        };
        TerminalLaunch { cwd, shell }
    }

    /// アクティブなターミナルにフォーカス（キー入力を受ける）。
    fn focus_active_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_dock.update(cx, |dock, cx| dock.focus_active(window, cx));
    }

    /// 下ドック（ターミナル）を開閉する。開くときは生成 + フォーカス（キー入力を受ける）。
    fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.show_bottom = !self.show_bottom;
        if self.show_bottom {
            self.focus_active_terminal(window, cx);
        }
        cx.notify();
    }

    /// アクティブなエディタタブを閉じて隣へ移る（⌘W / タブの ×）。
    fn close_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        self.close_tab_at(self.active_tab, window, cx);
    }

    /// `index` 番目のタブを閉じ、アクティブを隣へ寄せる。閉じたファイルは ⌘⇧T 用に履歴へ積み、
    /// LSP には didClose を送る。最後の 1 枚を閉じると空状態（分割も畳む）。
    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        // アクティブタブを閉じるなら ⌘F バー・hover は畳む（対象エディタが消える）。
        if index == self.active_tab {
            self.dismiss_buffer_search(cx);
            self.close_hover(cx);
        }
        let tab = self.tabs.remove(index);
        if !tab.transient {
            self.recently_closed_files.push(tab.path.clone());
            self.lsp_did_close(&tab.path);
        }
        // hot exit: タブを閉じる＝未保存編集の破棄（現仕様）なのでスナップショットも消す。
        if let Some(storage) = self.storage.clone() {
            let path = tab.path.clone();
            cx.background_executor()
                .spawn(async move {
                    let _ = storage.remove_hot_exit(&path);
                })
                .detach();
        }
        // active を有効域へ寄せる。
        if self.tabs.is_empty() {
            self.active_tab = 0;
            self.split_editor = None; // 何も無ければ分割（比較ビュー）も畳む
        } else if index < self.active_tab {
            self.active_tab -= 1;
        } else if index == self.active_tab {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        }
        // 新しいアクティブタブへフォーカス + 診断反映。
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.selected = self.tabs.get(self.active_tab).map(|tab| tab.path.clone());
        }
        self.sync_active_slot();
        self.push_active_diagnostics(cx);
        self.refresh_git_status(cx);
        self.save_state();
        cx.notify();
    }

    /// `index` 番目のタブをアクティブにする（タブクリック・⌘{ / ⌘}・重複オープン時）。
    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        // 別のタブへ移るなら ⌘F バー・hover は畳む（エディタ毎の状態）。
        if index != self.active_tab {
            self.dismiss_buffer_search(cx);
            self.close_hover(cx);
        }
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        self.active_tab = index;
        let editor = tab.editor.clone();
        let path = tab.path.clone();
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.selected = Some(path);
            slot.active_file = index;
        }
        self.push_active_diagnostics(cx);
        self.save_state();
        cx.notify();
    }

    /// タブを `from` から `to` へ移動する（ドラッグ並べ替え。active は同じタブを指し続ける）。
    fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if from >= count || to >= count || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        // active が指すタブを追従させる（remove→insert のインデックスずれを補正）。
        self.active_tab = if self.active_tab == from {
            to
        } else {
            let mut active = self.active_tab;
            if from < active {
                active -= 1;
            }
            if to <= active {
                active += 1;
            }
            active
        };
        self.sync_active_slot();
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
        let Some(path) = self.active_tab_path() else {
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
        if let Some(editor) = self.active_editor() {
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
    // ── ツリーのファイル操作（M10。local のみ・remote は M13 の Host 拡張と一緒に） ──

    /// インライン命名を開始する（新規ファイル/フォルダ = base の中 or 横・リネーム = target の名前）。
    fn start_naming(&mut self, kind: NamingKind, base: PathBuf, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (parent, target, initial) = match kind {
            NamingKind::Rename => {
                let parent = base.parent().map(Path::to_path_buf).unwrap_or_else(|| base.clone());
                let name = base.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                (parent, Some(base), name)
            }
            _ => {
                let parent = if is_dir {
                    base
                } else {
                    base.parent().map(Path::to_path_buf).unwrap_or(base)
                };
                (parent, None, String::new())
            }
        };
        // 親フォルダを展開しておく（入力行が見えるように）。
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.expanded.insert(parent.clone());
            slot.refresh();
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.explorer_naming = Some(ExplorerNaming { kind, parent, target, value: initial, focus });
        self.hide_context_menu(cx);
        cx.notify();
    }

    /// インライン命名の確定（Enter）。作成/リネームを実行してツリーを更新する。
    fn confirm_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(naming) = self.explorer_naming.take() else {
            return;
        };
        let name = naming.value.trim();
        if name.is_empty() || name.contains('/') {
            eprintln!("名前が不正: {name:?}");
            cx.notify();
            return;
        }
        let destination = naming.parent.join(name);
        let result = match naming.kind {
            NamingKind::NewFile => project::create_file_local(&destination),
            NamingKind::NewDir => project::create_dir_local(&destination),
            NamingKind::Rename => match &naming.target {
                Some(target) => {
                    // 開いているタブはリネーム前に閉じる（旧パスへの保存＝ファイル復活を防ぐ・v1）。
                    if let Some(index) = self.tabs.iter().position(|tab| &tab.path == target) {
                        self.close_tab_at(index, window, cx);
                    }
                    project::rename_local(target, &destination)
                }
                None => Ok(()),
            },
        };
        match result {
            Ok(()) => {
                if naming.kind == NamingKind::NewFile {
                    self.open_file(destination.clone(), window, cx);
                }
                if let Some(slot) = self.projects.get_mut(self.active) {
                    slot.selected = Some(destination);
                    slot.refresh();
                }
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("ファイル操作に失敗: {error:#}"),
        }
        cx.notify();
    }

    /// インライン命名の中止（Esc・外側クリック）。
    fn cancel_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.explorer_naming.take().is_some() {
            match self.active_editor() {
                Some(editor) => {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
                None => window.focus(&self.focus_handle, cx),
            }
            cx.notify();
        }
    }

    /// 命名入力のキー処理（検索パネルと同じ手書き流儀）。
    fn on_naming_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.cancel_naming(window, cx),
            "enter" => self.confirm_naming(window, cx),
            "backspace" => {
                if let Some(naming) = self.explorer_naming.as_mut() {
                    naming.value.pop();
                    cx.notify();
                }
            }
            "v" if event.keystroke.modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    if let Some(naming) = self.explorer_naming.as_mut() {
                        naming.value.push_str(text.trim());
                        cx.notify();
                    }
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }
                if let Some(naming) = self.explorer_naming.as_mut() {
                    naming.value.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    /// 複製（`name copy.ext`）→ ツリー更新。
    fn duplicate_entry(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        match project::duplicate_local(&path) {
            Ok(copy) => {
                if let Some(slot) = self.projects.get_mut(self.active) {
                    slot.selected = Some(copy);
                    slot.refresh();
                }
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("複製に失敗: {error:#}"),
        }
        cx.notify();
    }

    /// OS のゴミ箱へ（完全削除はしない）。開いているタブは先に閉じる。
    fn trash_entry(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.close_tab_at(index, window, cx);
        }
        match project::trash_local(&path) {
            Ok(()) => {
                if let Some(slot) = self.projects.get_mut(self.active) {
                    if slot.selected.as_ref() == Some(&path) {
                        slot.selected = None;
                    }
                    slot.refresh();
                }
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("ゴミ箱に入れられない: {error:#}"),
        }
        cx.notify();
    }

    fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.explorer_context_menu.take().is_some() {
            cx.notify();
        }
    }

    /// フォルダを**新規ウィンドウ**でプロジェクトとして開く（ウィンドウモデルの核）。
    /// レール ＋: ネイティブのフォルダ選択ダイアログ → 選んだフォルダを**このウィンドウのレールへ追加**。
    fn add_project_via_dialog(&mut self, cx: &mut Context<Self>) {
        // 多重起動ガード: 既にダイアログが出ていれば無視（＋連打で Finder を何枚も開かない）。
        if self.add_project_dialog_open {
            return;
        }
        self.add_project_dialog_open = true;
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(i18n::t!("rail.add_prompt"))),
        });
        cx.spawn(async move |workspace, cx| {
            let result = receiver.await;
            // 成功・キャンセル・失敗の全経路でフラグを戻す（早期 return で戻し忘れない）。
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.add_project_dialog_open = false;
                if let Ok(Ok(Some(paths))) = result {
                    if let Some(path) = paths.into_iter().next() {
                        workspace.add_project_slot(path, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// フォルダをレールの新しいプロジェクト slot として足す（既にあれば切替のみ）。
    /// ＋ ダイアログ経由: ローカルフォルダをレールへ追加（既にあれば切替のみ）。
    fn add_project_slot(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.open_folder_in_rail(host::LocalHost::shared(), path, None, cx);
    }

    /// フォルダを**このウィンドウのレール**に開く（既にあれば切替のみ）。新窓は作らない
    /// — ブランチ/worktree の既定導線（新窓はレール右クリック→「新しいウィンドウで開く」の明示操作・M10-2）。
    /// `branch` を渡すと「リンク worktree タブ」として記録し、右クリックの worktree/ブランチ削除を出す。
    /// 同じリポジトリの別ブランチは identity 色が親と衝突しがち → 使用中でない色に倒して方向感覚を保つ。
    fn open_folder_in_rail(
        &mut self,
        host: Arc<dyn Host>,
        path: PathBuf,
        branch: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) =
            self.projects.iter().position(|slot| slot.worktree.root() == path.as_path())
        {
            self.pending_project_switch = Some(index);
            cx.notify();
            return;
        }
        let worktree = match Worktree::with_host(host, &path) {
            Ok(worktree) => worktree,
            Err(error) => {
                self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx);
                return;
            }
        };
        let identity = read_project_identity(worktree.root());
        // identity 色があっても既にレールで使われていれば、未使用のパレット色へ倒す。
        let color = match identity.0 {
            Some(color) if !self.color_in_use(color) => color,
            _ => self.next_free_color(),
        };
        let mut slot = ProjectSlot {
            name: worktree.name().into(),
            branch: None,
            color,
            worktree: Rc::new(worktree),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            open_files: Vec::new(),
            active_file: 0,
            current_dir: None,
            icon: identity.1,
            worktree_branch: branch,
            dir_listings: std::cell::RefCell::new(HashMap::new()),
        };
        slot.refresh();
        let index = self.projects.len();
        self.projects.push(slot);
        // switch_project は window が要る（subscribe 経由に無い）ため、次の render で消化する。
        self.pending_project_switch = Some(index);
        cx.notify();
    }

    /// この色が既にレールのどれかのスロットで使われているか（色衝突の判定・小さな誤差を許容）。
    fn color_in_use(&self, color: Hsla) -> bool {
        self.projects.iter().any(|slot| colors_close(slot.color, color))
    }

    /// レールで未使用のパレット色（無ければスロット数で回す）。同色 2 枚を避けて方向感覚を保つ。
    fn next_free_color(&self) -> Hsla {
        (0..theme_core::IDENTITY_PALETTE_HEXES.len())
            .map(project_color)
            .find(|color| !self.color_in_use(*color))
            .unwrap_or_else(|| project_color(self.projects.len()))
    }

    // ── レール項目の右クリックメニュー（M10-2） ──

    /// レール項目の右クリックメニューを開く（色スウォッチ + 新規窓 / 外す / worktree・ブランチ削除）。
    fn open_rail_menu(&mut self, index: usize, position: Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.color_picker = None;
        self.rail_menu = Some(RailMenuState { project_index: index, position, confirm: None });
        cx.notify();
    }

    fn close_rail_menu(&mut self, cx: &mut Context<Self>) {
        if self.rail_menu.take().is_some() {
            cx.notify();
        }
    }

    /// スロットを**レールから外す**（表示のみ。ディスク・ブランチ・worktree は無傷＝安全側）。
    /// アクティブを外したら隣のスロットへビューを張り替える。最後の1枚は残す。
    fn remove_project_slot(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.rail_menu = None;
        if self.projects.len() <= 1 {
            self.push_toast(
                SharedString::from(i18n::t!("rail.cannot_remove_last")),
                self.accent(),
                cx,
            );
            cx.notify();
            return;
        }
        if index >= self.projects.len() {
            return;
        }
        let was_active = index == self.active;
        self.projects.remove(index);
        // active index を詰める（後ろの要素が 1 つ前へずれる。ロジックは純関数でテスト済み）。
        self.active = active_index_after_removal(self.active, index, self.projects.len());
        if was_active {
            // アクティブを外した → 新しいアクティブスロットのビュー（タブ/LSP/端末/git/監視）へ張り替える。
            self.load_active_slot(window, cx);
        }
        self.save_state();
        cx.notify();
    }

    /// レール右クリック「worktree を削除」: `git worktree remove`（強制）→ スロットも外す。
    fn remove_slot_worktree(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_slot_worktree_impl(index, false, window, cx);
    }

    /// レール右クリック「worktree ごとブランチを削除」: worktree remove → `git branch -D` → スロットも外す。
    fn delete_slot_branch(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_slot_worktree_impl(index, true, window, cx);
    }

    /// worktree（+任意でブランチ）を消してレールから外す共通経路。背景で git を叩き完了後にスロットを外す。
    /// `git worktree remove` は対象ツリーの中からは実行できないため、メイン作業ツリーの dir で叩く。
    fn delete_slot_worktree_impl(
        &mut self,
        index: usize,
        also_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rail_menu = None;
        // レール最後の1枚の worktree を消すと空レール＋ディスク破壊になる → 事前に断る（安全側）。
        if self.projects.len() <= 1 {
            self.push_toast(
                SharedString::from(i18n::t!("rail.cannot_remove_last")),
                self.accent(),
                cx,
            );
            cx.notify();
            return;
        }
        let Some(slot) = self.projects.get(index) else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let host = slot.worktree.host().clone();
        let target = slot.worktree.root().to_path_buf();
        let branch = slot.worktree_branch.clone();
        self.git_busy = true;
        cx.notify();
        let target_for_id = target.clone();
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    // メイン作業ツリー（一覧の先頭 = 対象以外）の dir から remove を叩く。
                    let main = project::git_worktrees_on(host.as_ref(), &target)
                        .into_iter()
                        .map(|worktree| worktree.path)
                        .find(|path| *path != target)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "メインの作業ツリーは削除できません（レールから外すを使ってください）"
                            )
                        })?;
                    project::remove_worktree_on(host.as_ref(), &main, &target, true)?;
                    if also_branch {
                        if let Some(branch) = branch.as_deref() {
                            project::delete_branch_on(host.as_ref(), &main, branch, true)?;
                        }
                    }
                    Ok::<(Option<String>, bool), anyhow::Error>((branch, also_branch))
                })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                workspace.git_busy = false;
                match result {
                    Ok((branch, also_branch)) => {
                        let message = if also_branch {
                            i18n::t!("git.branch_deleted", "branch" => branch.unwrap_or_default())
                        } else {
                            i18n::t!("git.worktree_removed")
                        };
                        workspace.push_toast(SharedString::from(message), workspace.accent(), cx);
                        if let Some(index) = workspace
                            .projects
                            .iter()
                            .position(|slot| slot.worktree.root() == target_for_id.as_path())
                        {
                            workspace.remove_project_slot(index, window, cx);
                        }
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
        self.open_source_as_window(source, cx);
        self.explorer_context_menu = None;
        cx.notify();
    }

    /// ProjectSource を新しいウィンドウで開く（ローカル folder / SSH の共通経路）。
    fn open_source_as_window(&mut self, source: ProjectSource, cx: &mut Context<Self>) {
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
    }

    /// パスをクリップボードへコピー。
    fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        self.explorer_context_menu = None;
        cx.notify();
    }

    /// Finder で表示（親フォルダを開いて選択・ローカルのみ）。
    fn reveal_in_finder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(error) = project::reveal_in_finder_local(path) {
            eprintln!("Finder 表示に失敗: {error:#}");
        }
        self.hide_context_menu(cx);
    }

    /// OS の既定アプリで開く（ファイル=関連付けアプリ / フォルダ=Finder・ローカルのみ）。
    fn open_with_default_app(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(error) = project::open_with_default_app_local(path) {
            eprintln!("既定アプリで開けない: {error:#}");
        }
        self.hide_context_menu(cx);
    }

    /// ファイルを開く（⌘P・ツリークリック・検索ジャンプ・F12 等の対話経路）。
    /// **読み込みは背景スレッド**（remote は 30s ブロックしうる — ARCHITECTURE §9）。
    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // 既に開いていれば重複タブを作らず、そのタブへ切り替える。
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let host = worktree.host().clone();
        let read_path = path.clone();
        cx.spawn(async move |_workspace, cx| {
            let content = cx
                .background_executor()
                .spawn(async move { host.read_file(&read_path) })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| match content {
                Ok(content) => workspace.open_loaded_file(path, content, window, cx),
                Err(error) => eprintln!("ファイルを開けない: {error:#}"),
            });
        })
        .detach();
    }

    /// ファイルを**同期で**開く（起動復元・レール/ブランチ切替の `open_slot_files` 専用）。
    /// 対話経路は [`Self::open_file`]（背景読み込み）。同期版はタブの並び順を保つために残す。
    fn open_file_sync(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let content = match worktree.host().read_file(&path) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("ファイルを開けない: {error:#}");
                return;
            }
        };
        self.open_loaded_file(path, content, window, cx);
    }

    /// ファイルを開いて（既に開いていれば切替えて）、**開き終わったエディタ**へ `apply` を実行する。
    /// open_file は背景読みなので「開いてからジャンプ」はこの合流点を使う（旧バッファへの誤 reveal 防止）。
    fn open_file_then(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut EditorView, &mut Context<EditorView>) + 'static,
    ) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |view, cx| apply(view, cx));
            }
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let host = worktree.host().clone();
        let read_path = path.clone();
        cx.spawn(async move |_workspace, cx| {
            let content = cx
                .background_executor()
                .spawn(async move { host.read_file(&read_path) })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| match content {
                Ok(content) => {
                    workspace.open_loaded_file(path, content, window, cx);
                    if let Some(editor) = workspace.active_editor() {
                        editor.update(cx, |view, cx| apply(view, cx));
                    }
                }
                Err(error) => eprintln!("ファイルを開けない: {error:#}"),
            });
        })
        .detach();
    }

    /// 読み込み済み内容からタブを開く（open_file / open_file_sync の合流点）。
    fn open_loaded_file(
        &mut self,
        path: PathBuf,
        content: host::FileContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 読み込み中に同じファイルが開かれていたら切り替えるだけ。
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            return;
        }
        // 新しいタブがアクティブになる ＝ ⌘F バー・hover は畳む。
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let buffer = match Buffer::from_content(worktree.host().clone(), &path, content) {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("ファイルを開けない: {error:#}");
                return;
            }
        };
        let theme = self.theme.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let editor = cx.new(|cx| EditorView::new(buffer, theme, accent, cx));
        // settings の実効化（M10-13）: font_size/tab_size/soft_wrap を適用（live 変更は observe_global）。
        {
            let current = settings::get(cx);
            let soft_wrap =
                current.soft_wrap || std::env::var_os("SHIRUSHI_SOFT_WRAP").is_some();
            let (font_size, tab_size) = (current.font_size, current.tab_size);
            editor.update(cx, |view, cx| {
                view.set_typography(font_size, tab_size, cx);
                view.set_soft_wrap(soft_wrap, cx);
            });
        }

        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        // 変更を監視（再描画 + LSP didChange）+ 確定入力（補完の自動トリガ）+ hover dwell を購読。タブごとに持つ。
        let observation = cx.observe(&editor, Self::on_editor_changed);
        let input_subscription = cx.subscribe_in(&editor, window, Self::on_editor_typed);
        let hover_subscription = cx.subscribe_in(&editor, window, Self::on_editor_hover);
        self.tabs.push(EditorTab {
            path: path.clone(),
            editor,
            transient: false,
            _observation: observation,
            _input_subscription: input_subscription,
            _hover_subscription: hover_subscription,
        });
        self.active_tab = self.tabs.len() - 1;

        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.selected = Some(path.clone());
        }
        self.sync_active_slot();
        self.refresh_git_status(cx);
        // LSP: この拡張子にサーバがあれば起動 + didOpen（初期化済みなら即 didOpen）。
        let has_language_server = self
            .active_editor()
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
                    open_file: None, // 旧形式は書かない（open_files に一本化）
                    open_files: slot.open_files.clone(),
                    active_file: slot.active_file,
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

    // ── オーバーレイ（Picker） ──

    /// ⌘P ファイルファインダ。全ファイル列挙は背景（大リポジトリの walk / remote RPC で UI を止めない）。
    fn open_file_finder(&mut self, _: &FileFinder, window: &mut Window, cx: &mut Context<Self>) {
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
                workspace.picker_files = files.into_iter().map(|(path, _)| path).collect();
                workspace.open_picker(PickerMode::Files, i18n::t!("finder.files"), items, window, cx);
            });
        })
        .detach();
    }

    /// ⌘⇧P コマンドパレット（M13）: 登録表 [`command_entries`] を名前+キー併記で並べ、
    /// 確定でアクションを dispatch する（閉じてから = エディタ/Workspace コンテキストで解決）。
    fn open_command_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        let sections = keymap_core::parse(keymap_core::DEFAULT_KEYMAP_JSON).unwrap_or_default();
        let items = command_entries()
            .iter()
            .enumerate()
            .map(|(id, (label_key, action_name))| {
                let mut item = PickerItem::new(id, i18n::t!(label_key));
                if let Some(keystrokes) = keymap_core::key_for_action(&sections, action_name) {
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
    fn open_project_switcher(&mut self, _: &ProjectSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_worktree_rows = Vec::new();
        let items = self.build_switcher_items(&[], cx).0;
        self.open_picker(PickerMode::Projects, i18n::t!("finder.projects"), items, window, cx);
        // 背景収集: 各プロジェクトの worktree 一覧 + それぞれの status（git 2 コマンド/worktree）。
        let sources: Vec<(usize, Arc<dyn Host>, PathBuf)> = self
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
                if workspace.picker_mode != PickerMode::Projects {
                    return;
                }
                let Some(picker) = workspace.picker.clone() else {
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
    fn build_switcher_items(
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
        for (index, slot) in self.projects.iter().enumerate() {
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
    fn open_worktree_target(
        &mut self,
        path: PathBuf,
        branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) =
            self.projects.iter().position(|slot| slot.worktree.root() == path.as_path())
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
    fn open_theme_selector(&mut self, _: &ThemeSelector, window: &mut Window, cx: &mut Context<Self>) {
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
        self.picker_themes = themes;
        self.theme_before_preview = Some(self.theme.clone());
        self.open_picker(PickerMode::Themes, i18n::t!("theme.picker_placeholder"), items, window, cx);
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
        // 全タブ + 分割ペインへ波及。
        for tab in &self.tabs {
            tab.editor.update(cx, |editor, cx| editor.set_theme(theme.clone(), cx));
        }
        if let Some(split) = &self.split_editor {
            split.update(cx, |editor, cx| editor.set_theme(theme.clone(), cx));
        }
        self.agent_panel.update(cx, |panel, cx| panel.set_theme(theme.clone(), cx));
        if let Some(picker) = &self.picker {
            picker.update(cx, |picker, cx| picker.set_theme(theme.clone(), cx));
        }
        self.terminal_dock
            .update(cx, |dock, cx| dock.set_theme(theme.clone(), cx));
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
                        if let Some((_, action_name)) = command_entries().get(id) {
                            match cx.build_action(action_name, None) {
                                Ok(action) => window.dispatch_action(action, cx),
                                Err(error) => eprintln!("パレット: {action_name} を解決できない: {error}"),
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
                                    let last_path = self.storage.as_ref().and_then(|storage| {
                                        storage.host_last_path(&alias).ok().flatten()
                                    });
                                    match last_path {
                                        Some(path) => self
                                            .connect_ssh_and_open(format!("ssh://{alias}{path}"), cx),
                                        None => self.open_ssh_input_seeded(
                                            format!("ssh://{alias}/"),
                                            window,
                                            cx,
                                        ),
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
                            if !self.show_right {
                                self.show_right = true; // Agent ドックを開く
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
        match self.active_editor() {
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
        self.record_nav_position(cx); // 検索ジャンプはナビ履歴へ（⌃- で戻れる）
        self.search_panel = None;
        self.open_file_then(path, window, cx, move |editor, cx| {
            editor.reveal_position(line, column, cx)
        });
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

    // ── ⌘F バッファ内検索/置換（M10） ──

    fn open_buffer_search(&mut self, _: &BufferSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_buffer_search_impl(false, window, cx);
    }

    fn open_buffer_replace(&mut self, _: &BufferReplace, window: &mut Window, cx: &mut Context<Self>) {
        self.open_buffer_search_impl(true, window, cx);
    }

    /// ⌘F / ⌥⌘F。閉じていれば現在位置を保存して開く（単一行の選択があればクエリ初期値に）。
    /// 開いていればフォーカスを付け直すだけ（⌥⌘F は置換行も出す）。
    fn open_buffer_search_impl(&mut self, with_replace: bool, window: &mut Window, cx: &mut Context<Self>) {
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
    fn close_buffer_search(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
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
    fn dismiss_buffer_search(&mut self, cx: &mut Context<Self>) {
        if self.buffer_search.take().is_some() {
            for tab in &self.tabs {
                tab.editor.update(cx, |editor, cx| editor.set_search_ranges(Vec::new(), cx));
            }
            cx.notify();
        }
    }

    /// ⌘F の検索を再計算する。(バッファ version, クエリ, トグル) が前回と同じならスキップ
    /// （エディタの observe は blink でも発火するため）。`reveal` = 現在マッチを選択して画面内へ。
    fn refresh_buffer_search(&mut self, reveal: bool, cx: &mut Context<Self>) {
        self.refresh_buffer_search_from(None, reveal, cx);
    }

    /// [`Self::refresh_buffer_search`] の anchor 指定版。`anchor` 以降で最初のマッチを現在マッチに
    /// する（None = 前回の現在マッチの開始位置 → 無ければエディタのキャレット位置）。
    fn refresh_buffer_search_from(
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
    fn reveal_current_buffer_match(&mut self, cx: &mut Context<Self>) {
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
    fn step_buffer_match(&mut self, delta: isize, cx: &mut Context<Self>) {
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
    fn replace_current_buffer_match(&mut self, cx: &mut Context<Self>) {
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
    fn replace_all_buffer_matches(&mut self, cx: &mut Context<Self>) {
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
    fn toggle_buffer_search_case(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.buffer_search.as_mut() {
            state.case_sensitive = !state.case_sensitive;
        }
        self.refresh_buffer_search(true, cx);
    }

    /// 正規表現トグル（⌘F バー。切替後は即再検索）。
    fn toggle_buffer_search_regex(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.buffer_search.as_mut() {
            state.is_regex = !state.is_regex;
        }
        self.refresh_buffer_search(true, cx);
    }

    /// ⌘F バーのキー入力（検索パネル・git パネルと同じ手書き入力の流儀）。
    fn on_buffer_search_key_down(
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
    pub fn debug_confirm_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.explorer_naming.is_some() {
            self.confirm_naming(window, cx);
        }
    }

    /// 開発用: (row,col) にキャレットを置いて rename を実行する（オフスクリーン検証）。
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
    pub fn debug_code_actions_probe(&mut self, row: usize, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.reveal_position(row, column, cx));
        }
        self.open_code_actions(&CodeActions, window, cx);
    }

    /// 開発用: ⌘. ポップアップの選択中アクションを確定して保存する（オフスクリーン検証）。
    pub fn debug_confirm_code_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_actions.is_some() {
            self.confirm_code_action(window, cx);
        }
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.save_now(cx));
        }
    }

    /// 開発用: (row,col) で ⇧F12 参照検索を実行する（オフスクリーン検証）。
    pub fn debug_references_probe(&mut self, row: usize, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.reveal_position(row, column, cx));
        }
        self.find_references(&FindReferences, window, cx);
    }

    /// ターミナルの file:line リンク（M13）。相対パスはアクティブプロジェクトの root 基準。
    /// subscribe に window が無いので pending_transient_tab と同様「次の render で消化」する。
    fn on_terminal_dock_event(
        &mut self,
        _dock: Entity<TerminalDock>,
        event: &TerminalDockEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalDockEvent::OpenPath { path, line } => {
                let resolved = if std::path::Path::new(path).is_absolute() {
                    PathBuf::from(path)
                } else if let Some(stripped) = path.strip_prefix("~/") {
                    std::env::home_dir()
                        .map(|home| home.join(stripped))
                        .unwrap_or_else(|| PathBuf::from(path))
                } else {
                    let Some(worktree) = self.active_worktree() else {
                        return;
                    };
                    worktree.root().join(path)
                };
                self.pending_terminal_jump = Some((resolved, line.saturating_sub(1)));
            }
            TerminalDockEvent::Dismissed => self.show_bottom = false,
        }
        cx.notify();
    }

    // ── リモート SSH の GUI 導線（M13） ──

    /// titlebar の SSH ボタン: `ssh://user@host/path` の入力バーを開く（goto/rename と同型）。
    /// seed は「アクティブが remote ならその URI・そうでなければ ssh://」。
    fn open_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self
            .active_slot()
            .and_then(|slot| {
                let host = slot.worktree.host();
                host.is_remote().then(|| {
                    format!("{}{}", host.display_name(), slot.worktree.root().display())
                })
            })
            .map(|identity| {
                // display_name は "SSH user@host" 形式 → ssh://user@host に直す。
                identity.replace("SSH ", "ssh://").replace(" ", "")
            })
            .unwrap_or_else(|| "ssh://".to_string());
        self.open_ssh_input_seeded(seed, window, cx);
    }

    /// SSH 入力バーを種文字列付きで開く（ホストピッカーからの遷移でも使う）。
    fn open_ssh_input_seeded(&mut self, seed: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.ssh_connecting {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.ssh_input = Some((seed, focus));
        cx.notify();
    }

    /// リモート SSH ホストピッカー（M13）: `~/.ssh/config` の Host 一覧 + 末尾に「手入力」。
    /// 選択で `ssh://<alias>/` を種に入力バーへ（パスだけ足して Enter で接続）。
    /// system OpenSSH に委ねるので User/HostName/鍵/ProxyJump は config のものがそのまま効く。
    /// 開発用: Agent タブの改名入力を開く（offscreen 検証・#4）。
    pub fn debug_tab_rename(&mut self, cx: &mut Context<Self>) {
        if !self.show_right {
            self.show_right = true;
        }
        self.agent_panel.update(cx, |panel, cx| panel.debug_start_rename(cx));
        cx.notify();
    }

    /// 開発用: スレッド履歴 Picker を開く（offscreen 検証・#5）。
    pub fn debug_open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_thread_history(&ThreadHistory, window, cx);
    }

    /// スレッド履歴を開く（#5）。DB の全スレッド（アーカイブ含む・updated_at 降順）を Picker に出す。
    /// 行頭●= スレッド色・detail = プロジェクト / ⎇ branch / トークン累計。確定で復元してアクティブに。
    fn open_thread_history(&mut self, _: &ThreadHistory, window: &mut Window, cx: &mut Context<Self>) {
        let Some(storage) = self.storage.clone() else {
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

    fn open_ssh_host_picker(&mut self, _: &RemoteSsh, window: &mut Window, cx: &mut Context<Self>) {
        let hosts = host::ssh_config_hosts();
        // 2階層: 上=最近のリモートプロジェクト（履歴・直接接続・#5）、下=config ホスト、末尾=手入力。
        let recent = self
            .storage
            .as_ref()
            .and_then(|storage| storage.recent_remote_projects().ok())
            .unwrap_or_default();
        let mut items: Vec<PickerItem> = Vec::new();
        let mut recent_uris: Vec<String> = Vec::new();
        for (host_key, path, name, _opened_at) in recent.iter().take(20) {
            let id = recent_uris.len();
            items.push(
                PickerItem::new(id, name.clone())
                    .with_accent(self.accent()) // 行頭●= 最近のプロジェクトの目印
                    .with_detail(format!("{host_key}:{path}")),
            );
            recent_uris.push(format!("ssh://{host_key}{path}"));
        }
        let recent_count = recent_uris.len();
        // config ホスト（id = recent_count + index。選ぶと前回パス即接続 or パス入力）。
        for (offset, host) in hosts.iter().enumerate() {
            let mut item = PickerItem::new(recent_count + offset, host.alias.clone());
            let base = match (&host.user, &host.hostname) {
                (Some(user), Some(hostname)) => Some(format!("{user}@{hostname}")),
                (None, Some(hostname)) => Some(hostname.clone()),
                (Some(user), None) => Some(format!("{user}@{}", host.alias)),
                (None, None) => None,
            };
            // 前回パスがあれば併記（→ が即接続先・#2d）。
            let last_path = self
                .storage
                .as_ref()
                .and_then(|storage| storage.host_last_path(&host.alias).ok().flatten());
            let detail = match (base, last_path) {
                (Some(base), Some(path)) => Some(format!("{base}  →{path}")),
                (Some(base), None) => Some(base),
                (None, Some(path)) => Some(format!("→{path}")),
                (None, None) => None,
            };
            if let Some(detail) = detail {
                item = item.with_detail(detail);
            }
            items.push(item);
        }
        // 末尾は「手入力」= 生の ssh:// 入力バー（config に無いホストへの逃げ道）。
        items.push(
            PickerItem::new(recent_count + hosts.len(), i18n::t!("ssh.manual_entry"))
                .with_detail("ssh://user@host/path"),
        );
        self.picker_ssh_recent = recent_uris;
        self.picker_ssh_hosts = hosts;
        self.open_picker(
            PickerMode::SshHosts,
            i18n::t!("ssh.picker_placeholder"),
            items,
            window,
            cx,
        );
    }

    fn close_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ssh_input.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    fn on_ssh_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_ssh_input(window, cx),
            "enter" => {
                let uri = self
                    .ssh_input
                    .as_ref()
                    .map(|(value, _)| value.trim().to_string())
                    .unwrap_or_default();
                self.close_ssh_input(window, cx);
                if !uri.is_empty() && uri != "ssh://" {
                    self.connect_ssh_and_open(uri, cx);
                }
            }
            "backspace" => {
                if let Some((value, _)) = self.ssh_input.as_mut() {
                    value.pop();
                    cx.notify();
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }
                if let Some((value, _)) = self.ssh_input.as_mut() {
                    value.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    /// SSH 接続 →（成功したら）新しいウィンドウで開く。接続は ControlMaster + server 配備で
    /// 数秒〜かかるため背景で行い、失敗はトーストで返す。system OpenSSH に委ねるので
    /// ~/.ssh/config の Host エイリアス・鍵・ProxyJump・agent がそのまま効く。
    fn connect_ssh_and_open(&mut self, uri: String, cx: &mut Context<Self>) {
        self.ssh_connecting = true;
        self.push_toast(
            SharedString::from(i18n::t!("ssh.connecting", "uri" => uri.clone())),
            self.accent(),
            cx,
        );
        cx.notify();
        // 成功したらホスト別に前回パスを記録（#2d・次回は打たずに即接続）。
        let last_path = host::SshProject::parse(&uri)
            .ok()
            .map(|project| (project.host, project.path.to_string_lossy().to_string()));
        let storage = self.storage.clone();
        cx.spawn(async move |workspace, cx| {
            let source = cx
                .background_executor()
                .spawn(async move {
                    let project = host::SshProject::parse(&uri)?;
                    let server_command = std::env::var("SHIRUSHI_REMOTE_SERVER_COMMAND")
                        .unwrap_or_else(|_| "shirushi-remote-server".to_string());
                    let remote = host::RemoteHost::connect_ssh(&project, &server_command)?;
                    let root = remote.root().to_path_buf();
                    Ok::<ProjectSource, anyhow::Error>(ProjectSource::new(remote, root))
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.ssh_connecting = false;
                match source {
                    Ok(source) => {
                        if let (Some(storage), Some((host_key, path))) = (&storage, &last_path) {
                            let _ = storage.set_host_last_path(host_key, path);
                            // 履歴（最近のリモートプロジェクト）にも記録（#5・2階層ピッカー）。
                            // name = パス末尾のフォルダ名（無ければホスト名）。
                            let name = std::path::Path::new(path)
                                .file_name()
                                .map(|component| component.to_string_lossy().to_string())
                                .filter(|component| !component.is_empty())
                                .unwrap_or_else(|| host_key.clone());
                            let _ = storage.record_remote_project(host_key, path, &name);
                        }
                        workspace.open_source_as_window(source, cx);
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

    /// SSH 入力バー（rename/goto と同型の中央上オーバーレイ）。
    fn render_ssh_input(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (value, focus) = self.ssh_input.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let display: SharedString = SharedString::from(value.clone());
        Some(
            div()
                .absolute()
                .top(px(96.))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(460.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(30.))
                        .px(px(10.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(accent)
                        .rounded(px(8.))
                        .shadow(vec![gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4))
                            .blur_radius(px(16.))])
                        .track_focus(focus)
                        .on_key_down(cx.listener(Self::on_ssh_key_down))
                        .text_size(px(12.5))
                        .text_color(theme.fg0)
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("ssh.label"))),
                        )
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(display))
                        .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent)),
                )
                .into_any_element(),
        )
    }

    // ── 自動アップデート（M13） ──

    /// 起動しばらく後に GitHub Releases を確認する（背景・失敗は静かに無視）。
    /// スクショ/プローブ実行時と `SHIRUSHI_NO_UPDATE_CHECK` ではネットへ出ない。
    fn schedule_update_check(&self, cx: &mut Context<Self>) {
        if std::env::var_os("SHIRUSHI_NO_UPDATE_CHECK").is_some()
            || std::env::var_os("SHIRUSHI_SCREENSHOT").is_some()
        {
            return;
        }
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(90))
                .await;
            let found = cx
                .background_executor()
                .spawn(async move { updater::check_for_update(env!("CARGO_PKG_VERSION")) })
                .await;
            if let Some(info) = found {
                let _ = workspace.update(cx, |workspace, cx| {
                    workspace.update_status = Some((info, UpdateState::Available));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// statusbar チップのクリック: ダウンロード → 署名検証 → 差し替え（背景）。
    fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some((info, state)) = self.update_status.clone() else {
            return;
        };
        if state != UpdateState::Available {
            return;
        }
        self.update_status = Some((info.clone(), UpdateState::Installing));
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater::download_and_install(&info).map(|_| info) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match result {
                Ok(info) => {
                    workspace.update_status = Some((info, UpdateState::Ready));
                    cx.notify();
                }
                Err(error) => {
                    workspace.update_status = None;
                    workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// 開発用: SSH 入力バーを開く（M13 の描画検証）。
    pub fn debug_open_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ssh_input(window, cx);
    }

    /// 開発用: SSH ホストピッカーを開く（SHIRUSHI_SSH_HOST_PROBE の描画検証・M13）。
    pub fn debug_open_ssh_host_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ssh_host_picker(&RemoteSsh, window, cx);
    }

    /// 開発用: ターミナルを開いて file:line リンクのクリック相当イベントを発火（M13 の結線検証）。
    pub fn debug_terminal_link(&mut self, path: String, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_terminal(&ToggleTerminal, window, cx);
        self.terminal_dock
            .update(cx, |dock, cx| dock.emit_open_path(path, line, cx));
    }

    /// 開発用: ⌘O スイッチャーを開く（M12-12 のオフスクリーン検証）。
    pub fn debug_open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_project_switcher(&ProjectSwitcher, window, cx);
    }

    /// 開発用: ⌘⇧P を開き、query で絞り込む（M13 のオフスクリーン検証）。
    /// `confirm` なら先頭候補を確定 = 実際にアクションが dispatch されるところまで通す。
    pub fn debug_palette_probe(
        &mut self,
        query: String,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_palette(&CommandPalette, window, cx);
        let Some(picker) = self.picker.clone() else {
            return;
        };
        picker.update(cx, |picker, cx| {
            if !query.is_empty() {
                picker.set_query(query, cx);
            }
            if confirm {
                picker.confirm_selected(cx);
            }
        });
    }

    /// 開発用: Todo ボードを開く（M12-10 のオフスクリーン検証）。
    /// `SHIRUSHI_TODOS_PLAN=1` なら ✨今日の計画も発火、`SHIRUSHI_TODOS_SEND=<line>` なら
    /// その行を ▶ で AI へ送る（受入「チェックがひとりでに入る」の自動 round trip）。
    pub fn debug_open_todo_board(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.todo_board.is_none() {
            self.toggle_todo_board(&ToggleTodoBoard, window, cx);
        }
        if std::env::var("SHIRUSHI_TODOS_PLAN").is_ok_and(|value| value == "1") {
            self.run_daily_plan(cx);
        }
        if let Ok(line) = std::env::var("SHIRUSHI_TODOS_SEND") {
            if let Ok(line) = line.parse::<usize>() {
                // 板の読み込み（bg）完了を待ってから該当行を送る。
                let Some(handle) = window.window_handle().downcast::<Workspace>() else {
                    return;
                };
                cx.spawn(async move |_workspace, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    let _ = handle.update(cx, |workspace, _window, cx| {
                        let text = workspace.todo_board.as_ref().and_then(|board| {
                            board
                                .items
                                .iter()
                                .find(|item| item.line == line)
                                .map(|item| item.text.clone())
                        });
                        match text {
                            Some(text) => {
                                eprintln!("TODOS_PROBE: ▶ 送信 line={line} text={text}");
                                workspace.send_todo_to_agent(line, text, cx);
                            }
                            None => eprintln!("TODOS_PROBE: line={line} が見つからない"),
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// 開発用: 全選択 → ⌘I → 指示を流し込んで実行（M12-8 のオフスクリーン検証）。
    /// `accept` なら提案到着をポーリングして適用 + 保存まで行う（受入の自動 round trip）。
    pub fn debug_inline_probe(
        &mut self,
        instruction: String,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |view, cx| {
            let end = view.buffer().len_bytes();
            view.select_byte_range(0..end, cx);
        });
        self.open_inline_edit(&InlineEdit, window, cx);
        let Some(state) = self.inline_edit.as_mut() else {
            eprintln!("INLINE_PROBE: 開けなかった（エディタ無し/対象空）");
            return;
        };
        state.instruction = instruction;
        self.execute_inline_edit(window, cx);
        if !accept {
            return;
        }
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            // 500ms 間隔で最大 120s（claude -p は数秒〜十数秒かかる）。
            for _ in 0..240 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let done = handle
                    .update(cx, |workspace, window, cx| {
                        let Some(state) = workspace.inline_edit.as_ref() else {
                            return true; // 閉じられた
                        };
                        if let Some(error) = &state.error {
                            eprintln!("INLINE_PROBE: 失敗 {error}");
                            return true;
                        }
                        if state.proposal.is_some() {
                            workspace.accept_inline_edit(window, cx);
                            workspace.save_active(&SaveActive, window, cx);
                            eprintln!("INLINE_PROBE: 適用+保存した");
                            return true;
                        }
                        false
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// 開発用: アクティブファイルの diff タブを開く（オフスクリーン検証）。
    pub fn debug_open_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_diff_tab(&OpenDiff, window, cx);
    }

    /// 開発用: ⌘⇧O アウトラインを開く（オフスクリーン検証）。
    pub fn debug_outline_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_outline(&OutlineSymbols, window, cx);
    }

    /// 開発用: ⌥⇧F 相当のフォーマット→保存を実行する（オフスクリーン検証）。
    pub fn debug_format_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_format(true, window, cx);
    }

    /// 開発用: 復元バーの「復元」を押す（オフスクリーン検証。pending が無ければ何もしない）。
    pub fn debug_restore_hot_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hot_exit_pending.is_some() {
            if std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some() {
                eprintln!("hotexit: 自動復元を実行");
            }
            self.restore_hot_exit(window, cx);
        }
    }

    /// 開発用: (row, col) にキャレットを置いて ⌘K ⌘I 相当の hover を出す（オフスクリーン検証）。
    /// キャレット矩形は直近 paint 由来なので、移動後 1 拍おいてから hover を出す。
    pub fn debug_hover_probe(
        &mut self,
        row: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |view, cx| view.reveal_position(row, column, cx));
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                workspace.show_hover_at_caret(&ShowHover, window, cx);
            });
        })
        .detach();
    }

    /// 開発用: レール関連フローを実コードで駆動しオフスクリーン検証する（M10-2）。
    /// `open-branch:<name>` = ブランチを worktree としてレールに開く（新窓でなくレール）。
    /// `remove-active` = アクティブスロットをレールから外す（隣へビュー張り替えの確認）。
    pub fn debug_rail_probe(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(branch) = command.strip_prefix("open-branch:") {
            self.open_branch_worktree(branch.to_string(), cx);
        } else if command == "remove-active" {
            self.remove_project_slot(self.active, window, cx);
        }
    }

    /// 開発用: ⌘F バーをクエリ入りで開く（オフスクリーン検証）。`replace` があれば置換行も開く。
    pub fn debug_open_buffer_search(
        &mut self,
        query: String,
        replace: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_buffer_search_impl(replace.is_some(), window, cx);
        if let Some(state) = self.buffer_search.as_mut() {
            state.query = query;
            if let Some(replace) = replace {
                state.replace = replace;
            }
        }
        self.refresh_buffer_search(true, cx);
    }

    // ── 描画 ──

    /// レールのアクティビティアイコン 1 個（Lucide SVG・単色で theme 色に着色）。クリックは呼び出し側で付ける。
    fn rail_icon(
        &self,
        id: &'static str,
        icon: &'static str,
        tooltip: impl Into<SharedString>,
        color: Hsla,
    ) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .size(px(30.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            .child(svg().path(icon).size(px(17.)).text_color(color))
            .tooltip(Tooltip::text(tooltip, theme.clone()))
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.active;
        let accent = self.accent();
        let rail = settings::get(cx).rail; // アイコンの表示/非表示（settings 反応）
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
                let monogram = slot
                    .icon
                    .as_ref()
                    .map(|icon| icon.to_string())
                    .unwrap_or_else(|| slot.name.chars().next().unwrap_or('•').to_string());
                let name = slot.name.clone();
                div()
                    .id(("rail-project", index))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.open_rail_menu(index, event.position, cx);
                        }),
                    )
                    .size(px(30.))
                    .relative() // リモートバッジ（絶対配置）の基準
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
                    .when(slot.worktree.host().is_remote(), |element| {
                        // リモート slot の見分け（#2）: 右下に server バッジ（SSH 接続先の目印）。
                        element.child(
                            div()
                                .absolute()
                                .bottom(px(-3.))
                                .right(px(-3.))
                                .size(px(13.))
                                .rounded_full()
                                .bg(theme.bg0)
                                .border_1()
                                .border_color(color)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(svg().path("icons/server.svg").size(px(8.)).text_color(color)),
                        )
                    })
                    .tooltip(Tooltip::text(name, theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.switch_project(index, window, cx)),
                    )
            }))
            // ＋ = フォルダ選択ダイアログ → このウィンドウのレールへ追加（多重起動はガード済み）
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
                    .tooltip(Tooltip::text(i18n::t!("rail.add_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.add_project_via_dialog(cx)),
                    ),
            )
            .child(div().flex_1())
            // ── アクティビティアイコン（settings.rail で個別に表示/非表示・Lucide SVG）──
            .when(rail.explorer, |element| {
                element.child(
                    self.rail_icon(
                        "rail-explorer",
                        "icons/panel-left.svg",
                        i18n::t!("rail.explorer"),
                        // アクティブ（左ドックがエクスプローラ表示）ならプロジェクト色・でなければ淡色（VSCode 風）。
                        if self.show_left && self.todo_board.is_none() && self.git_panel.is_none() {
                            accent
                        } else {
                            theme.fg2
                        },
                    )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                // エクスプローラ（ファイルブラウザ）はレールの既定ビュー。
                                // Todo/git が出ていれば**それをクリアして**エクスプローラへ戻す
                                // （旧: show_left トグルのみ → Todo が居座り「開くと Todo」問題）。
                                // 既にエクスプローラなら従来どおり表示トグル。
                                if this.todo_board.is_some() || this.git_panel.is_some() {
                                    this.todo_board = None;
                                    this.git_panel = None;
                                    this.show_left = true;
                                } else {
                                    this.show_left = !this.show_left;
                                }
                                cx.notify();
                            }),
                        ),
                )
            })
            .when(rail.search, |element| {
                element.child(
                    self.rail_icon(
                        "rail-search",
                        "icons/search.svg",
                        i18n::t!("rail.search"),
                        if self.search_panel.is_some() { accent } else { theme.fg2 },
                    ).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.open_project_search(&ProjectSearch, window, cx)),
                    ),
                )
            })
            .when(rail.git, |element| {
                element.child(
                    self.rail_icon(
                        "rail-git",
                        "icons/git-branch.svg",
                        i18n::t!("rail.git"),
                        if self.git_panel.is_some() { accent } else { theme.fg2 },
                    ).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.toggle_git_panel(&ToggleGitPanel, window, cx)),
                    ),
                )
            })
            .when(rail.todos, |element| {
                // Todo ボード（.shirushi/todos.md・M12-10）。表示中（アクティブ）はプロジェクト色。
                let color = if self.todo_board.is_some() { accent } else { theme.fg2 };
                element.child(
                    self.rail_icon("rail-todos", "icons/square-check.svg", i18n::t!("rail.todos"), color)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.toggle_todo_board(&ToggleTodoBoard, window, cx)
                            }),
                        ),
                )
            })
            .when(rail.agent, |element| {
                element.child(
                    self.rail_icon(
                        "rail-agent",
                        "icons/sparkles.svg",
                        i18n::t!("rail.agent"),
                        // AI ドック（右）表示中はプロジェクト色・畳んでいれば淡色。
                        if self.show_right { accent } else { theme.fg2 },
                    )
                        // 新規スレッドではなく Agent パネルの開閉トグル（他のアクティビティアイコンと同じ所作）。
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.toggle_dock(Dock::Right, cx)),
                        ),
                )
            })
            .when(rail.terminal, |element| {
                element.child(
                    self.rail_icon(
                        "rail-terminal",
                        "icons/square-terminal.svg",
                        i18n::t!("rail.terminal"),
                        if self.show_bottom { accent } else { theme.fg2 },
                    )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.toggle_terminal(&ToggleTerminal, window, cx)),
                        ),
                )
            })
            .when(rail.remote, |element| {
                // リモート SSH（~/.ssh/config → ワンクリック接続・#2）。アクティブがリモートなら色付き。
                let is_remote_active = self
                    .active_slot()
                    .map(|slot| slot.worktree.host().is_remote())
                    .unwrap_or(false);
                element.child(
                    self.rail_icon(
                        "rail-remote",
                        "icons/server.svg",
                        i18n::t!("rail.remote"),
                        if is_remote_active { accent } else { theme.fg2 },
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_ssh_host_picker(&RemoteSsh, window, cx)
                        }),
                    ),
                )
            })
            // ⚙ 設定ホーム（中央に開く。Agents セットアップの入口・常時表示）。
            .child(
                self.rail_icon(
                    "rail-settings",
                    "icons/settings.svg",
                    i18n::t!("rail.settings"),
                    if self.show_settings { accent } else { theme.fg2 },
                )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            this.show_settings = !this.show_settings;
                            cx.notify();
                        }),
                    ),
            )
            .child(div().h(px(6.)))
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
            // エクスプローラを触った → ⌘W の宛先はエディタタブ（Agent 判定を下げる）。
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, _cx| this.agent_active = false))
            .child(self.render_explorer_header(slot, cx))
            .child(body)
            .child(self.render_explorer_footer(cx))
            .child(self.left_dock_resize_handle(cx))
    }

    /// 左ドックの右縁リサイズハンドル（explorer / todo / git 共通・幅は `explorer_width` を共有）。
    /// 置く先のコンテナは `.relative()`（絶対配置の基準）であること。3 ビューは排他表示なので
    /// 同 id が同時に 2 つ出ることはない。
    fn left_dock_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .id("left-dock-resize")
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
            )
    }

    /// ツリー表示（縦。従来）。行 = chevron + アイコン + 名前。
    /// インライン命名の入力行（ツリー内に splice する・M10 ファイル操作）。
    fn render_naming_row(&self, depth: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(naming) = self.explorer_naming.as_ref() else {
            return div().into_any_element();
        };
        let theme = self.theme.clone();
        let accent = self.accent();
        let icon = match naming.kind {
            NamingKind::NewDir => "▸",
            _ => " ",
        };
        let display: SharedString = SharedString::from(naming.value.clone());
        let focus = naming.focus.clone();
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(ROW_HEIGHT))
            .pl(px(8. + depth as f32 * INDENT))
            .pr(px(8.))
            .track_focus(&focus)
            .on_key_down(cx.listener(Self::on_naming_key_down))
            .child(div().flex_none().w(px(12.)).text_size(px(10.)).text_color(theme.fg2).child(icon))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .h(px(19.))
                    .px(px(6.))
                    .rounded(px(4.))
                    .bg(theme.bg1)
                    .border_1()
                    .border_color(accent)
                    .text_size(px(12.))
                    .text_color(theme.fg0)
                    .overflow_hidden()
                    .child(div().whitespace_nowrap().overflow_hidden().child(display))
                    .child(div().flex_none().w(px(1.5)).h(px(13.)).bg(accent)),
            )
            .into_any_element()
    }

    fn render_tree(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let color = slot.color;
        let selected = slot.selected.clone();
        let git_status = &self.git_status;
        let root = slot.worktree.root().to_path_buf(); // ドラッグ時の @メンション相対パス用
        // インライン命名（M10）: rename は対象行を入力行に置き換え、New* は親フォルダ行の直後
        // （親がルートなら先頭）に入力行を挿す。
        let naming_kind = self.explorer_naming.as_ref().map(|naming| naming.kind);
        let naming_parent = self.explorer_naming.as_ref().map(|naming| naming.parent.clone());
        let naming_target = self.explorer_naming.as_ref().and_then(|naming| naming.target.clone());
        let naming_at_root = naming_kind.is_some()
            && naming_kind != Some(NamingKind::Rename)
            && naming_parent.as_deref() == Some(root.as_path());
        let mut elements: Vec<gpui::AnyElement> = Vec::new();
        if naming_at_root {
            elements.push(self.render_naming_row(0, cx));
        }
        for (index, row) in slot.rows.iter().enumerate() {
            if naming_kind == Some(NamingKind::Rename) && naming_target.as_ref() == Some(&row.path) {
                elements.push(self.render_naming_row(row.depth, cx));
                continue;
            }
            elements.push(self.render_tree_row(slot, index, row, &theme, color, &selected, git_status, &root, cx));
            if naming_kind.is_some()
                && naming_kind != Some(NamingKind::Rename)
                && row.is_dir
                && naming_parent.as_ref() == Some(&row.path)
            {
                elements.push(self.render_naming_row(row.depth + 1, cx));
            }
        }
        div().flex_1().overflow_hidden().children(elements).into_any_element()
    }

    /// ツリーの 1 行（従来 render_tree のクロージャ本体を関数化・M10 ファイル操作で入力行と共存させるため）。
    #[allow(clippy::too_many_arguments)]
    fn render_tree_row(
        &self,
        _slot: &ProjectSlot,
        index: usize,
        row: &TreeRow,
        theme: &Theme,
        color: Hsla,
        selected: &Option<PathBuf>,
        git_status: &HashMap<PathBuf, StatusKind>,
        root: &Path,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = theme.clone();
        let selected = selected.clone();
        let root = root.to_path_buf();
        {
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
                    // エージェントが触ったファイル = スレッド色ドット（色リンク・M12-4）。
                    .when_some(
                        self.agent_touched.get(&row.path).copied(),
                        |element, color| {
                            element.child(div().flex_none().size(px(7.)).rounded_full().bg(color))
                        },
                    )
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
                    .into_any_element()
        }
    }

    /// アイコングリッド表示（現在フォルダの直下。フォルダはクリックで中に入る・ファイルは開く）。
    fn render_icons(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let dir = slot.current_dir.clone().unwrap_or_else(|| slot.worktree.root().to_path_buf());
        let entries = slot.listed_dir(&dir); // キャッシュ付き（render 中の FS/RPC は初回のみ）
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
                let entries = slot.listed_dir(dir); // キャッシュ付き（render 中の FS/RPC は初回のみ）
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
                    .tooltip(Tooltip::text(i18n::t!("explorer.up_folder"), theme.clone()))
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
                    .tooltip(Tooltip::text(i18n::t!("explorer.back_to_project"), theme.clone()))
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
        // アイコンは Lucide SVG。svg は親の text_color を継承しない（自分の style.text.color のみ参照）
        // ため、色は svg へ直接指定。ホバー時の明るさ変化は group_hover で id 単位に効かせる。
        let button = |view: ExplorerView, id: &'static str, icon: &'static str, tip: String| {
            let active = current == view;
            let icon_color = if active { theme.fg0 } else { theme.fg2 };
            div()
                .id(id)
                .group(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(px(5.))
                .cursor_pointer()
                .when(active, |element| element.bg(theme.bg3))
                .hover(|style| style.bg(theme.bg3))
                .child(
                    svg()
                        .path(icon)
                        .size(px(15.))
                        .text_color(icon_color)
                        .group_hover(id, |style| style.text_color(theme.fg0)),
                )
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
            .child(button(ExplorerView::Tree, "view-tree", "icons/list.svg", i18n::t!("explorer.view_tree")))
            .child(button(ExplorerView::Columns, "view-columns", "icons/columns-3.svg", i18n::t!("explorer.view_columns")))
            .child(button(ExplorerView::Icons, "view-icons", "icons/layout-grid.svg", i18n::t!("explorer.view_icons")))
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

        let item = move |id: &'static str, label: String| {
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

        let is_local = self
            .active_worktree()
            .map(|worktree| !worktree.host().is_remote())
            .unwrap_or(false);
        if is_dir {
            let open_path = path.clone();
            menu_box = menu_box.child(item("ctx-open-window", i18n::t!("explorer.ctx_open_window")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.open_folder_as_window(open_path.clone(), cx)),
            ));
        } else {
            let open_path = path.clone();
            menu_box = menu_box.child(item("ctx-open", i18n::t!("explorer.ctx_open")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.open_file(open_path.clone(), window, cx);
                    this.hide_context_menu(cx);
                }),
            ));
        }
        // ── 既定アプリで開く / Finder で表示（ローカルのみ・シングルクリックで代替できない操作） ──
        if is_local {
            let open_path = path.clone();
            menu_box = menu_box.child(item("ctx-open-default", i18n::t!("explorer.ctx_open_default")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.open_with_default_app(&open_path, cx)),
            ));
            let reveal_path = path.clone();
            menu_box = menu_box.child(item("ctx-reveal", i18n::t!("explorer.ctx_reveal")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.reveal_in_finder(&reveal_path, cx)),
            ));
        }
        // ── ファイル操作（M10・local のみ） ──
        if is_local {
            let base = path.clone();
            menu_box = menu_box.child(item("ctx-new-file", i18n::t!("explorer.ctx_new_file")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.start_naming(NamingKind::NewFile, base.clone(), is_dir, window, cx)
                }),
            ));
            let base = path.clone();
            menu_box = menu_box.child(item("ctx-new-dir", i18n::t!("explorer.ctx_new_dir")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.start_naming(NamingKind::NewDir, base.clone(), is_dir, window, cx)
                }),
            ));
            let base = path.clone();
            menu_box = menu_box.child(item("ctx-rename", i18n::t!("explorer.ctx_rename")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.start_naming(NamingKind::Rename, base.clone(), is_dir, window, cx)
                }),
            ));
            let base = path.clone();
            menu_box = menu_box.child(item("ctx-duplicate", i18n::t!("explorer.ctx_duplicate")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.duplicate_entry(base.clone(), cx)),
            ));
            let base = path.clone();
            menu_box = menu_box.child(item("ctx-trash", i18n::t!("explorer.ctx_trash")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.trash_entry(base.clone(), window, cx)),
            ));
        }
        let copy_path = path.clone();
        menu_box = menu_box.child(item("ctx-copy", i18n::t!("explorer.ctx_copy_path")).on_mouse_down(
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
            SharedString::from(i18n::t!("searchpanel.placeholder"))
        } else {
            SharedString::from(state.query.clone())
        };
        let query_color = if state.query.is_empty() { theme.fg2 } else { theme.fg0 };

        // トグルチップ（Aa=大小区別 / .*=正規表現）。アクティブはアクセント面。
        let case_active = state.case_sensitive;
        let regex_active = state.is_regex;
        let toggle = |id: &'static str, label: &'static str, active: bool, tip: String| {
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
            _ if state.running => SharedString::from(i18n::t!("searchpanel.searching")),
            Some(error) => error.clone(),
            None if state.results_query.is_some() => {
                SharedString::from(i18n::t!("searchpanel.results", "n" => state.total_matches(), "files" => state.results.len()))
            }
            None => SharedString::from(i18n::t!("searchpanel.hint_enter")),
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
                    .child(SharedString::from(i18n::t!("searchpanel.truncated", "n" => SEARCH_MAX_ROWS)))
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
                                .child(toggle("search-case", "Aa", case_active, i18n::t!("searchpanel.case_tip")).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.toggle_search_case(cx)
                                    }),
                                ))
                                .child(toggle("search-regex", ".*", regex_active, i18n::t!("searchpanel.regex_tip")).on_mouse_down(
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
                                .child(SharedString::from(i18n::t!("searchpanel.hint_search")))
                                .child(SharedString::from(i18n::t!("searchpanel.hint_select")))
                                .child(SharedString::from(i18n::t!("searchpanel.hint_close"))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// hot exit の復元/破棄バー（起動時に前回の未保存スナップショットが見つかったとき・M10）。
    fn render_hot_exit_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let pending = self.hot_exit_pending.as_ref()?;
        let theme = self.theme.clone();
        let count = pending.len();
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_none()
                .h(px(20.))
                .px(px(8.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .border_1()
                .border_color(theme.warn.alpha(0.5))
                .text_size(px(11.))
                .text_color(theme.fg0)
                .cursor_pointer()
                .hover(|style| style.bg(theme.warn.alpha(0.18)))
                .child(SharedString::from(label))
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .px(px(12.))
                .flex_none()
                .bg(theme.warn.alpha(0.12))
                .border_b_1()
                .border_color(theme.warn.alpha(0.4))
                .text_size(px(11.5))
                .text_color(theme.fg0)
                .child(SharedString::from(format!(
                    "⚠ {}（{count}）",
                    i18n::t!("hotexit.pending")
                )))
                .child(div().flex_1())
                .child(button("hotexit-restore", i18n::t!("hotexit.restore")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.restore_hot_exit(window, cx)),
                ))
                .child(button("hotexit-discard", i18n::t!("hotexit.discard")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.discard_hot_exit(cx)),
                ))
                .into_any_element(),
        )
    }

    /// 外部変更の警告バー（dirty バッファにディスク変更が来たとき・M10 watch）。
    /// 上書きは絶対にせず、ユーザーに「再読込 / このまま」を選ばせる。
    fn render_external_change_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let editor = self.active_editor()?;
        if !editor.read(cx).is_externally_changed() {
            return None;
        }
        let theme = self.theme.clone();
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_none()
                .h(px(20.))
                .px(px(8.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .border_1()
                .border_color(theme.warn.alpha(0.5))
                .text_size(px(11.))
                .text_color(theme.fg0)
                .cursor_pointer()
                .hover(|style| style.bg(theme.warn.alpha(0.18)))
                .child(SharedString::from(label))
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .px(px(12.))
                .flex_none()
                .bg(theme.warn.alpha(0.12))
                .border_b_1()
                .border_color(theme.warn.alpha(0.4))
                .text_size(px(11.5))
                .text_color(theme.fg0)
                .child(SharedString::from(format!("⚠ {}", i18n::t!("watch.external_changed"))))
                .child(div().flex_1())
                .child(button("external-reload", i18n::t!("watch.reload")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        if let Some(editor) = this.active_editor() {
                            editor.update(cx, |view, cx| view.reload_from_disk(cx));
                        }
                    }),
                ))
                .child(button("external-keep", i18n::t!("watch.keep")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        if let Some(editor) = this.active_editor() {
                            editor.update(cx, |view, cx| view.dismiss_external_change(cx));
                        }
                    }),
                ))
                .into_any_element(),
        )
    }

    /// hover ポップアップ（LSP hover 結果・M10）。アンカーの上（入らなければ下）にコード字で出す。
    /// フォーカスは取らない。occlude なのでポップアップ上にマウスがある間は消えない。
    fn render_hover(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.hover.as_ref()?;
        let theme = self.theme.clone();
        const HOVER_LINE_HEIGHT: f32 = 17.0;
        let estimated_height = px(state.lines.len() as f32 * HOVER_LINE_HEIGHT + 18.0);
        // 原則アンカーの上。titlebar 等に食い込むなら下へ。
        let top = if state.position.y - estimated_height > px(70.) {
            state.position.y - estimated_height - px(8.)
        } else {
            state.position.y + px(20.)
        };
        let left = state.position.x.max(px(60.));
        Some(
            div()
                .absolute()
                .left(left)
                .top(top)
                .max_w(px(560.))
                .occlude()
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .rounded(px(8.))
                .px(px(10.))
                .py(px(7.))
                .shadow(vec![
                    gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
                ])
                .flex()
                .flex_col()
                .font_family("Guguru Sans Code")
                .text_size(px(11.5))
                .text_color(theme.fg1)
                .children(state.lines.iter().map(|line| {
                    let display = if line.is_empty() { SharedString::from(" ") } else { line.clone() };
                    div()
                        .h(px(HOVER_LINE_HEIGHT))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(display)
                }))
                .into_any_element(),
        )
    }

    /// LSP 補完ポップアップ（Ctrl-Space / 自動トリガ）。キャレット直下に prefix 絞り込み済み候補。
    /// 上下/Enter・Tab/Esc・印字キーは type-through で絞り込み継続。
    fn render_completion(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.completion.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let focus = state.focus.clone();
        let selected = state.selected;
        let filtered = state.filtered();

        let list = div().flex().flex_col().max_h(px(260.)).overflow_hidden().children(
            filtered.iter().take(12).enumerate().map(|(row, &index)| {
                let item = &state.items[index];
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
                .child(div().p(px(12.)).text_size(px(11.5)).text_color(fg2).child(SharedString::from(i18n::t!("git.no_project"))))
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
                    .child(SharedString::from(i18n::t!("git.title"))),
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
                            .tooltip(Tooltip::text(i18n::t!("git.pr_create_tip"), theme.clone()))
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
                            .tooltip(Tooltip::text(i18n::t!("git.pr_open_tip"), theme.clone()))
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
                    .tooltip(Tooltip::text(i18n::t!("git.new_branch_tip"), theme.clone()))
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
                    .tooltip(Tooltip::text(i18n::t!("git.close_tip"), theme.clone()))
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
            i18n::t!("git.branch_placeholder")
        } else {
            i18n::t!("git.message_placeholder")
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
                    .tooltip(Tooltip::text(i18n::t!("git.ai_message_tip"), theme.clone())),
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
                    .child(SharedString::from(i18n::t!("git.commit"))),
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
            body = body.child(self.git_section_header(&i18n::t!("git.staged"), staged_count, false, cx));
            for (index, change) in changes.iter().filter(|change| change.staged.is_some()).enumerate() {
                if let Some(kind) = change.staged {
                    body = body.child(self.git_change_row(change.path.clone(), kind, true, index, cx));
                }
            }
        }
        if unstaged_count > 0 {
            body = body.child(self.git_section_header(&i18n::t!("git.changes"), unstaged_count, true, cx));
            for (index, change) in changes.iter().filter(|change| change.unstaged.is_some()).enumerate() {
                if let Some(kind) = change.unstaged {
                    body = body.child(self.git_change_row(change.path.clone(), kind, false, index, cx));
                }
            }
        }
        if staged_count == 0 && unstaged_count == 0 {
            body = body.child(
                div().px(px(12.)).py(px(8.)).text_size(px(11.5)).text_color(fg2).child(SharedString::from(i18n::t!("git.no_changes"))),
            );
        }

        div()
            .w(px(self.explorer_width))
            .h_full()
            .flex()
            .flex_col()
            .flex_none()
            .relative() // リサイズハンドルの絶対配置基準
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
            .child(self.left_dock_resize_handle(cx))
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
        title: &str,
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
                        .tooltip(Tooltip::text(i18n::t!("git.stage_all_tip"), theme.clone()))
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
                        SharedString::from(if staged { i18n::t!("git.unstage") } else { i18n::t!("git.stage") }),
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
                .child(SharedString::from(i18n::t!("git.history"))),
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
                div().px(px(12.)).py(px(6.)).text_size(px(11.)).text_color(theme.fg2).child(SharedString::from(i18n::t!("git.no_commits"))),
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
            .child(div().px(px(8.)).py(px(4.)).text_size(px(10.5)).text_color(fg2).child(SharedString::from(i18n::t!("git.branches"))));

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
                            .tooltip(Tooltip::text(i18n::t!("git.worktree_open_tip"), theme.clone()))
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
                                .tooltip(Tooltip::text(i18n::t!("git.delete_branch_tip"), theme.clone()))
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
                let branch = worktree.branch.clone();
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
                        .child(div().flex_none().text_size(px(10.)).text_color(fg2).child(SharedString::from(i18n::t!("git.window_chip"))))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.open_worktree_window(path.clone(), branch.clone(), cx)
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
        // Peacock 相当: titlebar をアクティブプロジェクト色で淡く塗る（窓ごと識別・M13）。
        let accent = self.accent();
        let tint = gpui::Hsla { s: (accent.s + 0.12).min(1.0), a: 0.26, ..accent };
        div()
            .id("titlebar")
            .bg(tint)
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
            // リモート SSH で開く（M13・GUI 導線。~/.ssh/config のエイリアス/鍵/ProxyJump がそのまま効く）
            .child(
                self.rail_icon(
                    "titlebar-ssh",
                    "icons/server.svg",
                    i18n::t!("ssh.button_tip"),
                    if self.ssh_connecting { self.accent() } else { theme.fg2 },
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_ssh_host_picker(&RemoteSsh, window, cx)
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pr(px(4.))
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
                let status = if running { i18n::t!("git.running") } else { i18n::t!("git.idle") };
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

    /// プロジェクトピル: 枠 + 「名前 ▾」+「⎇ branch」。名前クリックで ⌘O。
    /// プロジェクト色はレール/キャレット等の許可箇所に集約（左縁チップは廃止・2026-07-17）。
    fn render_project_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let name = self
            .active_slot()
            .map(|slot| slot.name.clone())
            .unwrap_or_else(|| SharedString::from("—"));
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());

        let mut inner = div().flex().items_center().gap(px(11.)).py(px(4.)).px(px(11.)).child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg0)
                .child(format!("{name} ▾")),
        );
        if let Some(branch) = branch {
            inner = inner.child(
                div()
                    .id("branch-pill")
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_color(theme.fg1)
                    .text_size(px(11.5))
                    .rounded(px(4.))
                    .px(px(7.))
                    .py(px(2.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                    .child(div().text_color(theme.fg2).child("⎇"))
                    .child(SharedString::from(branch.to_string()))
                    .tooltip(Tooltip::text(i18n::t!("git.branch_menu_tip"), theme.clone()))
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
            .child(inner)
            .tooltip(Tooltip::text(i18n::t!("git.switcher_tip"), theme.clone()))
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
        let (id, label): (&'static str, String) = match dock {
            Dock::Left => ("dock-left", i18n::t!("dock.left")),
            Dock::Bottom => ("dock-bottom", i18n::t!("dock.bottom")),
            Dock::Right => ("dock-right", i18n::t!("dock.right")),
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

    /// 主ペインのタブ列（全タブ。クリック切替・× 閉じる・Chrome 風ドラッグ並べ替え・dirty ドット・
    /// git 色貫通）。M10 複数タブ。`agent_panel::render_thread_tabs` と同じ流儀。
    fn render_main_tabstrip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let active_tab = self.active_tab;
        div()
            .flex()
            .items_stretch()
            .h(px(TABSTRIP_HEIGHT))
            .flex_none()
            .bg(theme.bg0)
            .border_b_1()
            .border_color(theme.border)
            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                let is_active = index == active_tab;
                let name = tab
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| i18n::t!("tabs.untitled"));
                let dirty = tab.editor.read(cx).buffer().is_dirty();
                // タブ名も git 状態で色付け（ツリーと同じ色貫通）。
                let status = self.git_status.get(&tab.path).copied();
                let name_color = status.map(|status| Self::git_tint(&theme, status)).unwrap_or(theme.fg0);
                let drop_highlight = theme.bg2;
                let drag_name = SharedString::from(name.clone());
                let drag_theme = theme.clone();
                div()
                    .id(("editor-tab", index))
                    .flex()
                    .flex_col()
                    .h_full()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    // Zed 流の即時 hover。アクティブは常時 bg1。id で hover 再描画を保証。
                    .hover(|style| style.bg(theme.bg1))
                    .when(is_active, |element| element.bg(theme.bg1))
                    // アクティブタブ上線 = プロジェクト色（UI-SPEC §5）
                    .child(div().h(px(2.)).w_full().bg(if is_active { accent } else { theme.bg0 }))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px(px(14.))
                            .text_size(px(12.))
                            .text_color(if is_active { theme.fg0 } else { theme.fg1 })
                            .when(dirty, |element| {
                                element.child(div().size(px(7.)).rounded(px(3.5)).bg(theme.warn))
                            })
                            .child(div().text_color(name_color).child(SharedString::from(name)))
                            .child(
                                div()
                                    .id(("close-tab", index))
                                    .flex_none()
                                    .px(px(3.))
                                    .rounded(px(4.))
                                    .text_color(theme.fg2)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg2))
                                    .child("×")
                                    .tooltip(Tooltip::text(i18n::t!("tabs.close_tip"), theme.clone()))
                                    // × クリックはタブ切替へ伝播させない。
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.close_tab_at(index, window, cx);
                                        }),
                                    ),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.agent_active = false; // エディタ側を触った → ⌘W の宛先をタブへ
                            this.select_tab(index, window, cx);
                        }),
                    )
                    // Chrome 風ドラッグ並べ替え: タブを掴んで別タブ上で離すと順序が入れ替わる。
                    .on_drag(
                        DraggedEditorTab { index, name: drag_name, theme: drag_theme },
                        |dragged, _offset, _window, cx| cx.new(|_| dragged.clone()),
                    )
                    .drag_over::<DraggedEditorTab>(move |style, _dragged, _window, _cx| {
                        style.bg(drop_highlight)
                    })
                    .on_drop(cx.listener(move |this, dragged: &DraggedEditorTab, _window, cx| {
                        this.move_tab(dragged.index, index, cx);
                    }))
            }))
            .child(div().flex_1())
    }

    /// 右分割ペインのタブ列（単一比較ビュー。× = 分割を閉じる）。
    fn render_split_tabstrip(
        &self,
        editor: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let name = editor
            .read(cx)
            .buffer()
            .path()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| i18n::t!("tabs.untitled"));
        let dirty = editor.read(cx).buffer().is_dirty();
        div()
            .flex()
            .items_stretch()
            .h(px(TABSTRIP_HEIGHT))
            .flex_none()
            .bg(theme.bg0)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("split-tab")
                    .flex()
                    .flex_col()
                    .h_full()
                    .border_r_1()
                    .border_color(theme.border)
                    .bg(theme.bg1)
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
                            .child(SharedString::from(name))
                            .child(
                                div()
                                    .id("close-split")
                                    .flex_none()
                                    .px(px(3.))
                                    .rounded(px(4.))
                                    .text_color(theme.fg2)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg2))
                                    .child("×")
                                    .tooltip(Tooltip::text(i18n::t!("tabs.close_split_tip"), theme.clone()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| this.close_split(window, cx)),
                                    ),
                            ),
                    ),
            )
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

    /// ⌘F インライン検索/置換バー（エディタ右上に浮かせる・M10）。
    /// 行1 = [▸/▾] [クエリ] [n/m] [Aa] [.*] [‹] [›] [×]、行2（置換表示時）= [置換入力] [置換] [全置換]。
    fn render_buffer_search_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.buffer_search.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let show_replace = state.show_replace;
        let editing_replace = state.editing_replace && show_replace;

        // n/m カウンタ（正規表現エラーはここに赤で出す）。
        let (counter, counter_color) = if let Some(error) = &state.error {
            (error.clone(), theme.err)
        } else if state.matches.is_empty() {
            let text = if state.query.is_empty() {
                SharedString::from("")
            } else {
                SharedString::from(i18n::t!("search.no_results"))
            };
            (text, theme.fg2)
        } else {
            let text = format!(
                "{}/{}{}",
                state.current + 1,
                state.matches.len(),
                if state.truncated { "+" } else { "" }
            );
            (SharedString::from(text), theme.fg2)
        };

        // 入力フィールド（クエリ / 置換）。アクティブ側はアクセント枠 + 末尾キャレットバー。
        let field = |id: &'static str, text: &str, placeholder: String, active: bool| {
            let display: SharedString = if text.is_empty() {
                SharedString::from(placeholder)
            } else {
                SharedString::from(text.to_string())
            };
            let text_color = if text.is_empty() { theme.fg2 } else { theme.fg0 };
            div()
                .id(id)
                .flex_1()
                .flex()
                .items_center()
                .h(px(24.))
                .px(px(8.))
                .rounded(px(6.))
                .bg(theme.bg1)
                .border_1()
                .border_color(if active { accent } else { theme.border })
                .overflow_hidden()
                .cursor(CursorStyle::IBeam)
                .text_size(px(12.))
                .text_color(text_color)
                .child(div().overflow_hidden().whitespace_nowrap().child(display))
                .when(active, |element| {
                    element.child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent))
                })
        };

        // 小さな正方形ボタン（トグルチップ / マッチ移動 / 閉じる）。
        let chip = |id: &'static str, label: &'static str, active: bool, tip: SharedString| {
            div()
                .id(id)
                .flex()
                .flex_none()
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

        // 置換の実行ボタン（置換 / 全置換）。
        let action_button = |id: &'static str, label: String, tip: SharedString| {
            div()
                .id(id)
                .flex_none()
                .flex()
                .items_center()
                .h(px(22.))
                .px(px(8.))
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(label))
                .tooltip(Tooltip::text(tip, theme.clone()))
        };

        let query_row = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                // ▸/▾ = 置換行の開閉。
                div()
                    .id("bsearch-toggle-replace")
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.))
                    .h(px(24.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(if show_replace { "▾" } else { "▸" })
                    .tooltip(Tooltip::text(i18n::t!("search.toggle_replace"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            if let Some(state) = this.buffer_search.as_mut() {
                                state.show_replace = !state.show_replace;
                                if !state.show_replace {
                                    state.editing_replace = false;
                                }
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(
                field(
                    "bsearch-query",
                    &state.query,
                    i18n::t!("search.find_placeholder"),
                    !editing_replace,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        if let Some(state) = this.buffer_search.as_mut() {
                            state.editing_replace = false;
                        }
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(150.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .text_color(counter_color)
                    .child(counter),
            )
            .child(
                chip(
                    "bsearch-case",
                    "Aa",
                    state.case_sensitive,
                    SharedString::from(i18n::t!("search.case_sensitive")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.toggle_buffer_search_case(cx)),
                ),
            )
            .child(
                chip(
                    "bsearch-regex",
                    ".*",
                    state.is_regex,
                    SharedString::from(i18n::t!("search.use_regex")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.toggle_buffer_search_regex(cx)),
                ),
            )
            .child(
                chip("bsearch-prev", "‹", false, SharedString::from(i18n::t!("search.previous")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.step_buffer_match(-1, cx)),
                    ),
            )
            .child(
                chip("bsearch-next", "›", false, SharedString::from(i18n::t!("search.next")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.step_buffer_match(1, cx)),
                    ),
            )
            .child(
                chip("bsearch-close", "×", false, SharedString::from(i18n::t!("search.close")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_buffer_search(false, window, cx)),
                    ),
            );

        let replace_row = show_replace.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().flex_none().w(px(16.)))
                .child(
                    field(
                        "bsearch-replace",
                        &state.replace,
                        i18n::t!("search.replace_placeholder"),
                        editing_replace,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            if let Some(state) = this.buffer_search.as_mut() {
                                state.editing_replace = true;
                            }
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    action_button(
                        "bsearch-replace-one",
                        i18n::t!("search.replace_one"),
                        SharedString::from(i18n::t!("search.replace_one_tip")),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.replace_current_buffer_match(cx)),
                    ),
                )
                .child(
                    action_button(
                        "bsearch-replace-all",
                        i18n::t!("search.replace_all"),
                        SharedString::from(i18n::t!("search.replace_all_tip")),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.replace_all_buffer_matches(cx)),
                    ),
                )
        });

        let focus = state.focus.clone();
        Some(
            div()
                .id("buffer-search-bar")
                .absolute()
                .top(px(8.))
                .right(px(14.))
                .w(px(460.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(6.))
                .bg(theme.bg2)
                .rounded(px(8.))
                .border_1()
                .border_color(theme.border)
                .shadow(vec![
                    gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.35))
                        .blur_radius(px(18.)),
                ])
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_buffer_search_key_down))
                // バー内クリックはエディタへ通さない + フォーカスを付け直す（⌘W の宛先もエディタ側へ）。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.agent_active = false;
                        if let Some(state) = this.buffer_search.as_ref() {
                            let focus = state.focus.clone();
                            window.focus(&focus, cx);
                        }
                    }),
                )
                .child(query_row)
                .children(replace_row)
                .into_any_element(),
        )
    }

    /// 主ペイン（複数タブ列 + アクティブタブのパンくず + 本体）。⌘F バーは本体右上に浮かせる。
    fn render_main_pane(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(editor) = self.active_editor() else {
            return div().flex_1().into_any_element();
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(self.render_main_tabstrip(cx))
            .child(self.render_breadcrumb(&editor, cx))
            .children(self.render_external_change_bar(cx))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    .child(editor.clone())
                    .children(self.render_buffer_search_bar(cx)),
            )
            .into_any_element()
    }

    /// 右分割ペイン（単一タブ + パンくず + 本体）。
    fn render_split_pane(&self, editor: &Entity<EditorView>, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(self.render_split_tabstrip(editor, cx))
            .child(self.render_breadcrumb(editor, cx))
            .child(div().flex_1().overflow_hidden().child(editor.clone()))
            .into_any_element()
    }

    /// 設定ホーム（中央領域）。第1セクション = Agents セットアップ（M12）。
    /// 「ファイルが真実」: ここは settings.json への書き手にすぎない（既定選択＝persist）。
    /// 認証は各 CLI 側に委譲（ログイン/入れ方はターミナルで vendor コマンドを走らせるだけ・鍵は持たない）。
    fn render_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let default_agent = settings::get(cx).default_agent;
        let onboarding = !settings::get(cx).onboarded; // 初回はようこそ枠＋「これで始める」ボタン
        let mut rows = div().flex().flex_col().gap(px(6.));
        for (index, agent) in acp_client::AGENTS.iter().enumerate() {
            let is_default = agent.label == default_agent;
            // 3値ステータス（導入済み / npx 初回取得 / 未導入）。認証状態は見ない＝CLI 任せ。
            let (dot_color, status_text) = match agent.availability() {
                acp_client::Availability::Installed => (theme.ok, i18n::t!("settings.installed")),
                acp_client::Availability::Npx => (theme.warn, i18n::t!("settings.available_npx")),
                acp_client::Availability::Missing => (theme.fg2, i18n::t!("settings.not_installed")),
            };
            let default_control = if is_default {
                div()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .bg(accent.alpha(0.16))
                    .text_size(px(11.))
                    .text_color(accent)
                    .child(SharedString::from(i18n::t!("settings.is_default")))
                    .into_any_element()
            } else {
                let label = agent.label;
                div()
                    .id(("set-default", index))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(SharedString::from(i18n::t!("settings.make_default")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.set_default_agent(label, cx)),
                    )
                    .into_any_element()
            };
            let row = div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(9.))
                .rounded(px(8.))
                .bg(theme.bg2)
                .border_1()
                .border_color(if is_default { accent.alpha(0.5) } else { theme.border })
                .child({
                    // 実ブランドロゴ（Simple Icons/CC0・同梱）を text_color で着色。
                    // SI に無い Codex だけブランド色の頭文字モノグラムにフォールバック。
                    let (logo, mono, brand) = agent_brand(agent.id);
                    match logo {
                        Some(path) => div()
                            .flex_none()
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(svg().path(path).size(px(20.)).text_color(gpui::rgb(brand)))
                            .into_any_element(),
                        None => div()
                            .flex_none()
                            .size(px(26.))
                            .rounded(px(7.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(gpui::rgb(brand))
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(gpui::white())
                            .child(mono)
                            .into_any_element(),
                    }
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.fg0)
                                .child(agent.label),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(dot_color) // 導入状況の色（緑/琥珀/淡）
                                .child(SharedString::from(status_text)),
                        ),
                )
                .child(div().flex_1())
                .child(default_control)
                .child(self.agent_action_button(
                    ("agent-login", index),
                    i18n::t!("settings.login"),
                    agent.login_cmd,
                    cx,
                ))
                .child(self.agent_action_button(
                    ("agent-install", index),
                    i18n::t!("settings.install"),
                    agent.install_cmd,
                    cx,
                ));
            rows = rows.child(row);
        }

        let body = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .w_full()
            .max_w(px(680.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(if onboarding {
                                i18n::t!("settings.welcome_title")
                            } else {
                                i18n::t!("settings.title")
                            })),
                    )
                    .when(onboarding, |element| {
                        element.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("settings.welcome_sub"))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg1)
                            .child(SharedString::from(i18n::t!("settings.agents_heading"))),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("settings.agents_sub"))),
                    ),
            )
            .child(rows)
            .when(onboarding, |element| {
                // 初回の締め: 押すと onboarded=true・設定を閉じて紙吹雪。
                element.child(
                    div()
                        .id("onboarding-start")
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(38.))
                        .rounded(px(8.))
                        .bg(accent)
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.bg0)
                        .cursor_pointer()
                        .hover(|style| style.bg(accent.alpha(0.85)))
                        .child(SharedString::from(i18n::t!("settings.get_started")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.finish_onboarding(cx)),
                        ),
                )
            });

        div()
            .id("settings-scroll")
            .flex_1()
            .overflow_y_scroll()
            .bg(theme.bg1)
            .child(div().flex().justify_center().px(px(28.)).py(px(24.)).child(body))
            .into_any_element()
    }

    /// オンボーディング完了: 既定はもう選べているので `onboarded=true` にして設定を閉じ、紙吹雪で祝う。
    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        settings::set_user_value(cx, "onboarded", serde_json::Value::Bool(true));
        self.show_settings = false;
        self.celebrate_confetti(cx);
    }

    /// 祝いの紙吹雪を降らせる（~2.2s で自動的に止める）。
    fn celebrate_confetti(&mut self, cx: &mut Context<Self>) {
        self.confetti = true;
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(2200)).await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.confetti = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// 祝いの紙吹雪オーバーレイ（`confetti` が true の間だけ全面に色紙が降る）。
    /// 各粒子は同尺の with_animation で落下（delta² の重力）＋末尾フェード。位置は relative で窓非依存。
    fn render_confetti(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.confetti {
            return None;
        }
        let palette = [
            self.accent(),
            self.theme.ok,
            self.theme.warn,
            self.theme.err,
            project_color(1),
            project_color(3),
            project_color(5),
        ];
        let mut overlay = div().absolute().top_0().left_0().size_full();
        for i in 0..48usize {
            let color = palette[i % palette.len()];
            let x = (i as f32 * 0.618_034) % 1.0; // 黄金比で横に散らす
            let phase = i as f32;
            let wide = i % 2 == 0;
            let particle = div()
                .absolute()
                .w(px(if wide { 7. } else { 5. }))
                .h(px(if i % 3 == 0 { 10. } else { 6. }))
                .rounded(px(2.))
                .bg(color)
                .with_animation(
                    ("confetti", i),
                    Animation::new(std::time::Duration::from_millis(1900)),
                    move |element, delta| {
                        // 粒ごとに落下速度・開始高さをばらす（決定的な擬似乱数）＝帯にならず散らばる。
                        let speed = 0.65 + ((i * 37) % 100) as f32 / 100.0 * 0.7; // 0.65..1.35
                        let start_y = -0.2 - ((i * 53) % 100) as f32 / 100.0 * 0.25;
                        let t = (delta * speed).min(1.0);
                        let fall = start_y + (1.25 - start_y) * t * t; // 上から下へ（重力で加速）
                        let sway = (t * 9.0 + phase).sin() * 0.03; // 横揺れ
                        let opacity = (1.0 - ((t - 0.8) / 0.2).max(0.0)).clamp(0.0, 1.0);
                        element
                            .left(gpui::relative((x + sway).clamp(0.0, 1.0)))
                            .top(gpui::relative(fall))
                            .opacity(opacity)
                    },
                );
            overlay = overlay.child(particle);
        }
        Some(overlay.into_any_element())
    }

    /// 設定画面のアクションボタン（ログイン/入れ方）。押すと vendor コマンドをターミナルで実行。
    fn agent_action_button(
        &self,
        id: (&'static str, usize),
        text: String,
        command: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
            .child(SharedString::from(text))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.open_command_terminal(command, window, cx)),
            )
    }

    /// 既定エージェントを保存＋グローバル即更新（`set_user_value` が persist と in-memory 反映を両方やる）。
    /// **既定はここ（Settings）でしか変えない** — 哲学「自分で決めた既定は意図しない限り変わらない」。
    fn set_default_agent(&mut self, label: &str, cx: &mut Context<Self>) {
        settings::set_user_value(cx, "default_agent", serde_json::Value::String(label.to_string()));
        cx.notify();
    }

    /// 指定コマンドを新しいターミナルタブで実行し、終わったらログインシェルに落ちる（ログイン/導入用）。
    /// 各 CLI の認証はローカルで行う想定なので **ローカルシェル**で走らせる（remote 時も cwd は外す）。
    fn open_command_terminal(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self
            .active_slot()
            .filter(|slot| !slot.worktree.host().is_remote())
            .map(|slot| slot.worktree.root().to_path_buf());
        let shell = Some((
            "/bin/sh".to_string(),
            vec!["-lc".to_string(), format!("{command}; exec \"${{SHELL:-/bin/zsh}}\" -l")],
        ));
        self.show_bottom = true;
        self.terminal_dock.update(cx, |dock, cx| {
            dock.open_command(TerminalLaunch { cwd, shell }, window, cx)
        });
        cx.notify();
    }

    fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let content = if self.show_settings {
            // 設定ホーム（中央領域を占有・レール ⚙ で開閉）。第1セクション=Agents。
            self.render_settings(cx)
        } else if self.tabs.is_empty() {
            // 初回起動の案内（M13）: 最初の 4 手をキーバッジ付きで。10 分で使い始める導線。
            let hint = |key: &'static str, text: String| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(52.))
                            .flex()
                            .justify_center()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(10.5))
                            .text_color(theme.fg1)
                            .child(key),
                    )
                    .child(div().text_size(px(12.5)).text_color(theme.fg2).child(SharedString::from(text)))
            };
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(px(15.))
                        .text_color(theme.fg1)
                        .pb(px(6.))
                        .child(SharedString::from(i18n::t!("welcome.title"))),
                )
                .child(hint("⌘O", i18n::t!("welcome.open_project")))
                .child(hint("⌘P", i18n::t!("welcome.open_file")))
                .child(hint("⌘⇧A", i18n::t!("welcome.new_thread")))
                .child(hint("⌘⇧P", i18n::t!("welcome.palette")))
                .into_any_element()
        } else {
            let mut panes = div().flex_1().flex().min_h_0().child(self.render_main_pane(cx));
            // 右分割ペイン（あれば仕切り + 2 枚目）。
            if let Some(split) = self.split_editor.clone() {
                panes = panes
                    .child(div().w(px(1.)).flex_none().bg(theme.border))
                    .child(self.render_split_pane(&split, cx));
            }
            panes.into_any_element()
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .bg(theme.bg1)
            // エディタ側を触った → ⌘W の宛先をエディタタブに（Agent 判定を下げる）。
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, _cx| this.agent_active = false))
            .children(self.render_hot_exit_bar(cx))
            .child(content)
            // 下ドック（ターミナル）はエディタ列の下に積む（サイドドックには被らない）。
            .when(self.show_bottom, |element| element.child(self.render_bottom_dock(cx)))
    }

    fn render_bottom_dock(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        self.terminal_dock.clone()
    }

    fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        // Peacock 相当: statusbar をアクティブプロジェクト色で淡く塗る（窓ごと識別・M13）。
        let accent = self.accent();
        let tint = gpui::Hsla { s: (accent.s + 0.12).min(1.0), a: 0.26, ..accent };
        let branch = self.active_slot().and_then(|slot| slot.branch.clone());
        let remote_host = self.active_slot().and_then(|slot| {
            slot.worktree
                .host()
                .is_remote()
                .then(|| SharedString::from(slot.worktree.host().display_name().to_string()))
        });
        let change_count = self.git_status.len();
        let (cursor, language) = match self.active_editor() {
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
            // プロジェクト色スウォッチ（footer から色変更・M13）。常に「今の窓の色」が見え、クリックで色ピッカー（⌘K⌘C と同経路）。
            .child(
                div()
                    .id("statusbar-project-color")
                    .size(px(11.))
                    .rounded_full()
                    .bg(self.accent())
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .hover(|style| style.border_color(theme.fg2))
                    .tooltip(Tooltip::text(i18n::t!("cmd.project_color"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            // クリック位置の真上にピッカーを出す（footer から開くので左上へ飛ばさない）。
                            let anchor =
                                gpui::point(event.position.x, event.position.y - px(176.));
                            this.open_color_picker(this.active, anchor, window, cx)
                        }),
                    ),
            )
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
                        .tooltip(Tooltip::text(i18n::t!("terminal.git_tip"), theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.toggle_git_panel(&ToggleGitPanel, window, cx)
                            }),
                        ),
                )
            })
            // 権限待ちスレッドのドット（M12-5。裏の窓でも気づける）。クリックで Agent パネルへ。
            .when_some(self.waiting_thread.clone(), |element, (name, color)| {
                element.child(
                    div()
                        .id("statusbar-waiting-thread")
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg2))
                        .child(beacon_dot("waiting-pulse", color, true))
                        .child(div().text_color(theme.fg1).child(name))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                this.show_right = true;
                                this.agent_active = true;
                                cx.notify();
                            }),
                        ),
                )
            })
            .child(
                div()
                    .id("statusbar-diagnostics")
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .rounded(px(4.))
                    .px(px(4.))
                    .child(div().text_color(error_color).child(format!("✗ {errors}")))
                    .child(div().text_color(warning_color).child(format!("▲ {warnings}")))
                    // クリックで診断一覧（ファイル別・M11）。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_diagnostics_panel(&DiagnosticsPanel, window, cx)
                        }),
                    ),
            );

        let right = div()
            .flex()
            .items_center()
            .gap_3()
            // 自動アップデートのチップ（M13）: 新版あり → クリックで更新 → 再起動案内。
            .when_some(self.update_status.clone(), |element, (info, state)| {
                let (label, clickable) = match state {
                    UpdateState::Available => {
                        (i18n::t!("update.available", "version" => info.version.clone()), true)
                    }
                    UpdateState::Installing => (i18n::t!("update.installing"), false),
                    UpdateState::Ready => (i18n::t!("update.ready"), false),
                };
                element.child(
                    div()
                        .id("update-chip")
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .text_color(self.accent())
                        .when(clickable, |chip| {
                            chip.cursor_pointer()
                                .hover(|style| style.bg(theme.bg2))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| this.install_update(cx)),
                                )
                        })
                        .child(SharedString::from(label)),
                )
            })
            .when_some(cursor, |element, cursor| element.child(SharedString::from(cursor)))
            .child(SharedString::from("UTF-8"))
            .when_some(language, |element, language| element.child(language));

        div()
            .flex()
            .items_center()
            .h(px(STATUSBAR_HEIGHT))
            .px_3()
            .flex_none()
            .bg(tint)
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .child(left)
            .child(div().flex_1())
            .child(right)
    }
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
        match self.active_editor() {
            Some(editor) => editor.read(cx).focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

/// エージェントの識別マーク: (ブランドロゴ SVG パス or None, モノグラム fallback, 描画色)。
/// ロゴは ACP が広告しない（Zed もカタログ由来の同梱で ACP 経由ではない）ため UI 側で持つ。
/// 実ロゴは Simple Icons（CC0・商標は各社帰属・識別目的の nominative use）を同梱。SI に無い
/// もの（OpenAI/Codex）は頭文字にフォールバック。単色マーク（copilot/opencode/kimi）は淡色で描く。
fn agent_brand(id: &str) -> (Option<&'static str>, &'static str, u32) {
    match id {
        "claude" => (Some("icons/brand-claude.svg"), "C", 0xd9_77_57), // テラコッタ
        "codex" => (None, ">_", 0x10_a3_7f),                          // OpenAI マークは CC0 に無い→中立のコード記号（商標フリー）
        "copilot" => (Some("icons/brand-copilot.svg"), "Co", 0xd0_d5_db), // 単色→淡色
        "qwen" => (Some("icons/brand-qwen.svg"), "Q", 0x69_50_ef),
        "opencode" => (Some("icons/brand-opencode.svg"), "OC", 0xd0_d5_db),
        "kimi" => (Some("icons/brand-kimi.svg"), "K", 0xd0_d5_db),
        "grok" => (None, "G", 0x4b_55_63), // xAI マークは CC0 に無い→頭文字（公式 SVG を置けば差し替え可）
        _ => (None, "?", 0x88_88_88),
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 承認カードからの diff タブ要求を消化（イベント時点では window が無いため・M12-6）。
        if let Some((title, buffer)) = self.pending_transient_tab.take() {
            self.open_transient_tab(title, buffer, _window, cx);
        }
        // スレッド履歴を開く要求を消化（Agent パネルの履歴ボタン → window が要る・#5）。
        if self.pending_open_history {
            self.pending_open_history = false;
            self.open_thread_history(&ThreadHistory, _window, cx);
        }
        // ＋で追加したプロジェクトへの切替を消化（ダイアログ経由は window が無い）。
        if let Some(index) = self.pending_project_switch.take() {
            self.switch_project(index, _window, cx);
        }
        // ターミナル file:line リンクのジャンプを消化（M13）。
        if let Some((path, line)) = self.pending_terminal_jump.take() {
            self.record_nav_position(cx);
            self.open_file_then(path, _window, cx, move |editor, cx| {
                editor.reveal_position(line as usize, 0, cx);
            });
        }
        let theme = self.theme.clone();
        // 窓縁のプロジェクト色枠（方向感覚・Peacock 相当。面は塗らず縁の 2px 線のみ = UI-SPEC §1.3）。
        // リモートでは「今どのマシンか」を窓ごと一目で判る signal になる。
        let accent = self.accent();
        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_file_finder))
            .on_action(cx.listener(Self::open_project_switcher))
            .on_action(cx.listener(Self::open_project_search))
            .on_action(cx.listener(Self::open_buffer_search))
            .on_action(cx.listener(Self::open_buffer_replace))
            .on_action(cx.listener(Self::open_theme_selector))
            .on_action(cx.listener(Self::open_project_color))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::go_to_definition))
            .on_action(cx.listener(Self::trigger_completion))
            .on_action(cx.listener(Self::show_hover_at_caret))
            .on_action(cx.listener(Self::navigate_back))
            .on_action(cx.listener(Self::navigate_forward))
            .on_action(cx.listener(Self::open_goto_line))
            .on_action(cx.listener(Self::format_document))
            .on_action(cx.listener(Self::save_active))
            .on_action(cx.listener(Self::open_rename))
            .on_action(cx.listener(Self::open_inline_edit))
            .on_action(cx.listener(Self::toggle_todo_board))
            .on_action(cx.listener(Self::open_command_palette))
            .on_action(cx.listener(Self::open_ssh_host_picker))
            .on_action(cx.listener(Self::open_thread_history))
            .on_action(cx.listener(Self::open_code_actions))
            .on_action(cx.listener(Self::find_references))
            .on_action(cx.listener(Self::open_diff_tab))
            .on_action(cx.listener(|this, _: &NextHunk, _window, cx| this.step_hunk_header(1, cx)))
            .on_action(cx.listener(|this, _: &PrevHunk, _window, cx| this.step_hunk_header(-1, cx)))
            .on_action(cx.listener(Self::open_outline))
            .on_action(cx.listener(Self::open_workspace_symbols))
            .on_action(cx.listener(Self::next_diagnostic))
            .on_action(cx.listener(Self::prev_diagnostic))
            .on_action(cx.listener(Self::open_diagnostics_panel))
            .on_action(cx.listener(Self::toggle_split))
            .on_action(cx.listener(Self::toggle_git_panel))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::restore_closed_tab))
            .on_action(cx.listener(Self::select_next_tab))
            .on_action(cx.listener(Self::select_prev_tab))
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
            .border_2()
            .border_color(accent)
            // 窓の角丸(10px・UI-SPEC §1.4)に枠を沿わせる。四角い枠だと隅が窓の丸みでクリップされ細く見える。
            .rounded(px(10.))
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
                        // 左カラムは Todo ボード / git パネル / エクスプローラを切替（排他）。
                        let column = if self.todo_board.is_some() {
                            self.render_todo_board(cx)
                        } else if self.git_panel.is_some() {
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
            .children(self.render_hover(cx))
            .children(self.render_goto_line(cx))
            .children(self.render_rename_input(cx))
            .children(self.render_inline_edit(cx))
            .children(self.render_ssh_input(cx))
            .children(self.render_code_actions(cx))
            .children(self.render_hunk_menu(cx))
            .children(self.render_toasts(cx))
            .children(self.render_color_picker(cx))
            .children(self.render_rail_menu(cx))
            .children(self.render_branch_menu(cx))
            .children(self.render_explorer_context_menu(cx))
            .children(self.render_confetti(cx)) // 最前面（祝いの紙吹雪）
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn active_index_after_removal_shifts_correctly() {
        // [A,B,C,D] active=C(2)
        // 前を消す（A・0）→ 後ろが 1 つ前へ = index 1（C）。
        assert_eq!(active_index_after_removal(2, 0, 3), 1);
        // 後ろを消す（D・3）→ active 不変（C）。
        assert_eq!(active_index_after_removal(2, 3, 3), 2);
        // active 自身を消す（C・2、末尾でない）→ 同 index（次に繰り上がる D）。
        assert_eq!(active_index_after_removal(2, 2, 3), 2);
        // 末尾かつ active を消す（D・3 of [A,B,C,D]）→ 1 つ前（C=2）。
        assert_eq!(active_index_after_removal(3, 3, 3), 2);
        // 先頭 active を消す（A・0）→ 0（次に繰り上がる B）。
        assert_eq!(active_index_after_removal(0, 0, 3), 0);
        // 2 枚から active(1) を消す → 0。
        assert_eq!(active_index_after_removal(1, 1, 1), 0);
    }

    #[test]
    fn inline_edit_diff_lines_marks_changes_and_truncates() {
        // 変更を +/- で表示（受入: 「その場に diff 表示」の中身・M12-8）。
        let old_text = "fn get() -> u32 {\n    42\n}\n";
        let new_text = "fn get() -> Result<u32, Error> {\n    Ok(42)\n}\n";
        let lines = inline_edit_diff_lines(old_text, new_text, 14);
        assert!(lines.iter().any(|line| line.starts_with("-fn get() -> u32")));
        assert!(lines.iter().any(|line| line.starts_with("+fn get() -> Result")));
        // ファイルヘッダ（---/+++）は出さない。@@ は ··· に置換。
        assert!(!lines.iter().any(|line| line.starts_with("---") || line.starts_with("+++")));
        assert!(!lines.iter().any(|line| line.starts_with("@@")));

        // 同一テキストは「変更なし」。
        let same = inline_edit_diff_lines("a\n", "a\n", 14);
        assert_eq!(same.len(), 1);

        // 長い diff は中央省略で max_lines+? に収まる（head + 省略行 + tail = max_lines）。
        let old_long: String = (0..40).map(|i| format!("line{i}\n")).collect();
        let new_long: String = (0..40).map(|i| format!("changed{i}\n")).collect();
        let truncated = inline_edit_diff_lines(&old_long, &new_long, 14);
        assert_eq!(truncated.len(), 14);
        assert!(truncated.iter().any(|line| line.starts_with("··· (+")));
    }

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
    fn locations_group_by_file_with_preview_lines() {
        let dir = std::env::temp_dir().join(format!("shirushi_refs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.rs");
        std::fs::write(&file, "struct Thing;\nfn use_it(t: Thing) {}\n").unwrap();
        let uri = format!("file://{}", file.display());
        let value = json!([
            { "uri": uri, "range": { "start": {"line": 0, "character": 7}, "end": {"line": 0, "character": 12} } },
            { "uri": uri, "range": { "start": {"line": 1, "character": 13}, "end": {"line": 1, "character": 18} } }
        ]);
        let results = locations_to_file_matches(host::LocalHost::shared().as_ref(), &value);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].matches.len(), 2);
        assert_eq!(results[0].matches[0].line, 0);
        assert_eq!(results[0].matches[0].line_text, "struct Thing;");
        assert_eq!(results[0].matches[1].column, 13);
        assert_eq!(results[0].matches[1].byte_range.len(), 5); // "Thing"
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completion_trigger_classification() {
        use CompletionTrigger::*;
        // 識別子文字（英数・_）
        assert_eq!(classify_completion_trigger("a", "bufa"), Identifier);
        assert_eq!(classify_completion_trigger("_", "x_"), Identifier);
        assert_eq!(classify_completion_trigger("9", "v9"), Identifier);
        // `.` は常に新規トリガ
        assert_eq!(classify_completion_trigger(".", "buf."), Fresh);
        // `::` は 2 個目の `:` で発火（1 個目では出さない）
        assert_eq!(classify_completion_trigger(":", "theme_core:"), None);
        assert_eq!(classify_completion_trigger(":", "theme_core::"), Fresh);
        // IME 確定（複数文字・非 ASCII）や記号はトリガしない
        assert_eq!(classify_completion_trigger("日本語", "日本語"), None);
        assert_eq!(classify_completion_trigger(" ", "fn "), None);
        assert_eq!(classify_completion_trigger("(", "call("), None);
        // IME 確定でも末尾が識別子ならトリガ（"buf" をまとめて確定した場合）
        assert_eq!(classify_completion_trigger("buf", "buf"), Identifier);
    }

    #[test]
    fn completion_filter_narrows_by_prefix() {
        let item = |label: &str| CompletionItem {
            label: SharedString::from(label.to_string()),
            insert_text: label.to_string(),
            detail: None,
            kind: SharedString::from("fn"),
        };
        let items = vec![item("push"), item("push_str"), item("Pop"), item("insert")];
        assert_eq!(filter_completion_indices(&items, "pu"), vec![0, 1]);
        assert_eq!(filter_completion_indices(&items, "PO"), vec![2]); // 大小無視
        assert!(filter_completion_indices(&items, "zz").is_empty());
        assert_eq!(filter_completion_indices(&items, "").len(), 4);
    }
}
