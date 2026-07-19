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
use explorer::{
    ContextMenu as ExplorerContextMenu, Explorer, ExplorerProject, Naming as ExplorerNaming,
    NamingKind, TreeRow, ViewMode as ExplorerView,
};
use gpui::{
    Animation, AnimationExt, App, Bounds, ClipboardItem, Context, CursorStyle, Div, Entity,
    EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Point, SharedString, Stateful, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowControlArea, WindowOptions, actions, div, point, prelude::*,
    pulsating_between, px, size, svg,
};
use git_ui::{BranchMenu as BranchMenuState, GitPanel, RepositorySnapshot};
use host::Host;
use lang::lsp::{
    apply_text_edits_to_string, language_server_for, parse_definition, parse_hover_lines,
    parse_text_edits, parse_workspace_edit,
};
use project::{GraphCommit, ProjectSource, StatusKind, Worktree};
use search_ui::{SearchPanel, SearchPanelEvent};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut, Range};
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
    /// ⌘⇧P コマンドパレット（M13）。id は [`CommandRegistry`] の添字。
    Commands,
    /// リモート SSH ホストピッカー（M13）。id は `picker_ssh_hosts` の添字（末尾 id = 手入力）。
    SshHosts,
    /// スレッド履歴（過去スレッド一覧・#5）。id は `picker_history` の添字。
    ThreadHistory,
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

struct ProjectSlot {
    worktree: Rc<Worktree>,
    name: SharedString,
    /// 描画中に git process/RPC を起動しないための現在ブランチ cache。
    branch: Option<String>,
    color: Hsla,
    explorer: ExplorerProject,
    /// このプロジェクトで開いているタブのファイル一覧（左から順・M10 複数タブ）。
    /// アクティブプロジェクトでは `Workspace.tabs` が真実で、ここへは同期する（`sync_active_slot`）。
    /// 非アクティブプロジェクトでは復元用の記録（切替時に開き直す）。
    open_files: Vec<PathBuf>,
    /// アクティブタブの添字（`open_files` 内）。
    active_file: usize,
    /// `.shirushi/settings.json` の絵文字アイコン（M12-11。None = 頭文字モノグラム）。
    icon: Option<SharedString>,
    /// リンク worktree として開いたスロットのブランチ名（Some = worktree タブ・M10-2）。
    /// レール右クリックの「worktree を削除 / ブランチを削除」を出す判定に使う。通常/メインは None。
    worktree_branch: Option<String>,
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
        self.explorer.refresh(&self.worktree);
    }

    /// アイコン/カラム表示用のディレクトリ列挙（キャッシュ付き）。初回だけ FS/RPC を読み、
    /// 以後の render はキャッシュを返す（無効化は [`Self::refresh`]）。
    fn listed_dir(&self, dir: &Path) -> Vec<project::Entry> {
        self.explorer.listed_dir(dir)
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

/// Rail の project metadata と長寿命 session を同じ添字で管理する。
struct ProjectSessions {
    projects: Vec<ProjectSlot>,
    active: usize,
    sessions: Vec<ProjectSession>,
}

struct RepositoryController {
    status: HashMap<PathBuf, StatusKind>,
    refresh_generation: u32,
}

struct ChromeState {
    show_left: bool,
    show_right: bool,
    show_bottom: bool,
    show_settings: bool,
    settings_view: Entity<settings::SettingsView>,
    pending_settings_command: Option<String>,
    confetti: bool,
    agent_width: f32,
    resizing_agent: bool,
    resize_start_x: f32,
    resize_start_width: f32,
    explorer_width: f32,
    resizing_explorer: bool,
    should_move_window: bool,
}

struct WorkspaceOverlays {
    picker: Option<Entity<Picker>>,
    picker_mode: PickerMode,
    picker_files: Vec<PathBuf>,
    picker_themes: Vec<(SharedString, ThemeSource)>,
    theme_before_preview: Option<Theme>,
    picker_observation: Option<Subscription>,
    color_picker: Option<ColorPickerState>,
    rail_menu: Option<RailMenuState>,
    ssh_input: Option<(String, FocusHandle)>,
    ssh_connecting: bool,
    add_project_dialog_open: bool,
    pending_project_switch: Option<usize>,
}

struct NotificationCenter {
    toasts: Vec<(SharedString, Hsla, u32)>,
    toast_gen: u32,
}

struct WorkspacePersistence {
    state_path: Option<PathBuf>,
    storage: Option<storage::Storage>,
}

struct UpdateController {
    status: Option<(updater::UpdateInfo, UpdateState)>,
}

pub struct Workspace {
    project_sessions: ProjectSessions,
    theme: Theme,
    focus_handle: FocusHandle,
    chrome: ChromeState,
    overlays: WorkspaceOverlays,
    notifications: NotificationCenter,
    persistence: WorkspacePersistence,
    updater: UpdateController,
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

impl Deref for Workspace {
    type Target = ProjectSession;

    fn deref(&self) -> &Self::Target {
        &self.project_sessions
    }
}

impl DerefMut for Workspace {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.project_sessions
    }
}

include!("project_session.rs");
include!("commands.rs");
include!("panels.rs");
include!("git_controller.rs");
include!("editor_area/mod.rs");
include!("editor_area/language.rs");
include!("editor_area/inline_edit.rs");
include!("todo_panel.rs");
include!("editor_area/diff.rs");
include!("editor_area/diagnostics.rs");
include!("project_watcher.rs");
include!("rail.rs");
include!("notifications.rs");
include!("editor_area/hot_exit.rs");
include!("project_switch.rs");
include!("editor_area/tabs.rs");
include!("explorer_controller.rs");
include!("overlays.rs");
include!("dev_probes.rs");
include!("rail_view.rs");
include!("explorer_view.rs");
include!("editor_area/overlays.rs");
include!("git_view.rs");
include!("chrome.rs");

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

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(command) = self.chrome.pending_settings_command.take() {
            self.open_command_terminal(&command, _window, cx);
        }
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
        if let Some(index) = self.overlays.pending_project_switch.take() {
            self.switch_project(index, _window, cx);
        }
        // child Entity からの file:line ジャンプを消化。
        if let Some((path, line, column)) = self.pending_navigation.take() {
            self.record_nav_position(cx);
            self.open_file_then(path, _window, cx, move |editor, cx| {
                editor.reveal_position(line, column, cx);
            });
        }
        if let Some(path) = self.pending_open_git_diff.take() {
            self.open_diff_tab_for(path, None, _window, cx);
        }
        if let Some(hunk) = self.pending_stage_hunk.take() {
            self.stage_hunk(hunk, cx);
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
                    .when(self.chrome.show_left, |element| {
                        // 左カラムは Todo ボード / git パネル / エクスプローラを切替（排他）。
                        let column = if self.todo_panel.read(cx).open {
                            self.render_todo_board(cx)
                        } else if self.git_panel_open(cx) {
                            self.render_git_panel(cx)
                        } else {
                            self.render_explorer(cx).into_any_element()
                        };
                        element.child(column)
                    })
                    .child(self.render_center(cx))
                    .when(self.chrome.show_right, |element| element.child(self.render_agent_dock(cx))),
            )
            .child(self.render_statusbar(cx))
            // オーバーレイ（最前面）
            .when_some(self.overlays.picker.clone(), |this, picker| this.child(picker))
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
