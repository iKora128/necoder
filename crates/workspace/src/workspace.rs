//! workspace — レール（プロジェクト切替）+ 左ドックのエクスプローラ + 中央エディタ + ステータスバー。
//!
//! ARCHITECTURE §5: 1 窓 = アクティブな (project, branch)。レール = 窓内切替。UI-SPEC §1.3 の色の
//! 許可リストに従い、**プロジェクト色**はレール枠/リング・ツリー選択の左バー・キャレットにのみ流す。
//! 状態（開プロジェクト・アクティブ・開ファイル）は `state.json` に保存し、再起動で復元する。

pub(crate) use crate::persistence::{state_path, PersistedProject, PersistedState, RestoredTabs};
pub(crate) use crate::updater;
pub(crate) use agent_panel::AgentPanel;
pub(crate) use editor_core::{Buffer, Selection};
pub(crate) use editor_view::{
    ComposerEvent, EditorHoverEvent, EditorInputEvent, EditorView, PositionSnapshot,
};
pub(crate) use explorer::{
    ContextMenu as ExplorerContextMenu, Explorer, ExplorerProject, Naming as ExplorerNaming,
    NamingKind, TreeRow, ViewMode as ExplorerView,
};
pub(crate) use futures::StreamExt as _; // LSP 通知 pump の `.next()`
pub(crate) use git_ui::{BranchMenu as BranchMenuState, GitPanel, RepositorySnapshot};
pub(crate) use gpui::{
    actions, div, point, prelude::*, px, size, svg, Animation, AnimationExt, App, Bounds,
    ClipboardItem, Context, CursorStyle, Div, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Point, SharedString, Stateful, StyleRefinement, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowControlArea, WindowOptions,
};
pub(crate) use host::Host;
pub(crate) use lang::lsp::{
    apply_text_edits_to_string, language_server_for, parse_definition, parse_hover_lines,
    parse_text_edits, parse_workspace_edit,
};
pub(crate) use project::{GraphCommit, ProjectSource, StatusKind, Worktree};
pub(crate) use search_ui::{SearchPanel, SearchPanelEvent};
pub(crate) use std::collections::HashMap;
pub(crate) use std::ops::{Deref, DerefMut, Range};
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::rc::Rc;
pub(crate) use std::sync::Arc;
pub(crate) use terminal_view::{TerminalDock, TerminalDockEvent, TerminalLaunch};
pub(crate) use theme_core::{project_color, Theme, ThemeSource};
pub(crate) use ui::Tooltip;
pub(crate) use ui::{DraggedFile, Picker, PickerEvent, PickerItem};

// ── 子モジュール（Rust の descendant 可視性で hub の private にアクセス可能）──
mod chrome;
mod commands;
mod control_ipc;
mod control_transport;
mod control_view;
mod dev_probes;
mod explorer_controller;
mod explorer_view;
mod fleet_view;
mod git_controller;
mod git_view;
mod herd_view;
mod notifications;
mod overlays;
mod rail;
mod rail_view;
mod remote_ssh;
mod worktree_delete;
pub use control_ipc::control_socket_path;
// 制御 IPC の足回り（unix socket / 名前付きパイプ）。CLI 側（necoder の fleet.rs）も使う。
pub use control_transport::{ControlListener, ControlStream};
mod coordinator;
pub(crate) use coordinator::is_coordinator_thread_name;
mod todo_panel;
pub(crate) use todo_panel::*;
mod editor_area;
pub(crate) use editor_area::*;
mod panels;
mod project_session;
mod project_switch;
mod project_watcher;
pub(crate) use commands::*;
pub(crate) use panels::*;
pub(crate) use project_session::*;

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
        // Todo ボード（.necoder/todos.md・M12-10）。
        ToggleTodoBoard,
        // 編隊 herd サイドバー（状態一覧・M14）。
        ToggleHerdSidebar,
        // 編隊モード（全画面 = herd + 系譜グラフ + グリッド + ニュース・M14）。
        ToggleFleet,
        // solo で AI パネルだけを全画面に（2026-07-27）。エディタもエクスプローラも畳んで
        // チャット 1 枚にする＝「ファイルを開かずに全部 AI に任せる」使い方のための面。
        ToggleAgentFullScreen,
        // 管制タブ（編隊中央のダッシュボード・FLEET-CONTROL-PLAN P3）。編隊モードごと開く。
        ToggleControl,
        // 管制: 要対応キューの先頭へ没入（⏎・P3）。
        ControlNext,
        // ⌘⇧P コマンドパレット（M13）。
        CommandPalette,
        // バグ報告（GitHub new issue を環境情報つきで開く・M13 公開準備）。
        ReportBug,
        // 設定画面を開く（⌘, / メニュー「設定…」・M13 メニューバー）。
        OpenSettings,
        // 最近使った項目を開く…（ランチャー起動。Enter=レール / ⌘⏎=新窓・M13 メニューバー）。
        OpenRecent,
        // 開く…（ネイティブのフォルダ選択ダイアログ・M13 メニューバー）。
        OpenDialog,
        // バージョン表記のトースト（メニュー「necoder について」・M13 メニューバー）。
        About,
        // macOS 標準のアプリ/ウィンドウ操作（メニューバー用・M13。handlers は workspace root）。
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
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
        NextProject,
        PrevProject,
    ]
);

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum PickerMode {
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
    /// 「＋」統一オープン: 開く系アクション + 最近（local/remote 混在）。id は `picker_open_rows` の添字。
    OpenLauncher,
}

const RAIL_WIDTH: f32 = 46.0;
const DOCK_WIDTH: f32 = 218.0;
const DOCK_MIN: f32 = 150.0;
const DOCK_MAX: f32 = 640.0;
const ROW_HEIGHT: f32 = 23.0;
const INDENT: f32 = 14.0;
const AGENT_DOCK_WIDTH: f32 = 440.0; // Agent パネル既定幅（ドラッグで可変）
const AGENT_DOCK_MIN: f32 = 320.0;
const AGENT_DOCK_MAX: f32 = 900.0; // 幅を測れない場面のフォールバック上限
/// Agent ドックをどれだけ広げても中央エディタに最低これだけは残す（ウィンドウ相対の上限計算に使う）。
const MIN_CENTER_WIDTH: f32 = 360.0;
const RESIZE_HANDLE_WIDTH: f32 = 6.0;
/// 下段ドック（編隊のニュース/ターミナル・solo のターミナル）の既定と可動域。
/// 既定 = ターミナルが最初から使える高さ（旧・固定 240px を踏襲）。ニュース 5〜6 行に寄せた 132px は
/// ターミナルには浅すぎた（実測 ~8 行・ヘッダ込みで実質 6 行）ため引き上げた。上縁ドラッグで可変。
const BOTTOM_DOCK_HEIGHT: f32 = 240.0;
const BOTTOM_DOCK_MIN: f32 = 60.0;
const BOTTOM_DOCK_MAX: f32 = 900.0;
const TITLEBAR_HEIGHT: f32 = 38.0;
const TABSTRIP_HEIGHT: f32 = 34.0;
const BREADCRUMB_HEIGHT: f32 = 26.0;
const STATUSBAR_HEIGHT: f32 = 26.0;
/// macOS のネイティブ信号機（appears_transparent 時も残る）を避けるための左余白。
const TRAFFIC_LIGHT_INSET: f32 = 92.0; // ネイティブ信号機とプロジェクトピルの間に余白を持たせる

/// titlebar / statusbar のドックトグルが指すドック。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Dock {
    Left,
    Right,
    Bottom,
}

/// ペインに載る 1 タブ（M10 複数タブ）。当面は具体型（エディタ）だけ・多態化は必要時に育てる
/// （ARCHITECTURE §3 の Pane/Item 初版）。`path` はタブの同一判定と永続化のキー。
pub(crate) struct EditorTab {
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
}

/// 編隊セルの ⋯ メニュー（2026-07-27）。「× を押しても消えない」「消すのと画面から外すの違いが
/// 分からない」への回答 — **同じ場所に全段を並べ、各行に「何が残るか」を書く**。
struct FleetCellMenuState {
    /// 対象セルの `fleet_cells` 添字。
    cell: usize,
    position: Point<gpui::Pixels>,
}

/// ⌘F バッファ内検索の表示上限。これを超えるマッチは n/m に「+」を付けて数えない
/// （全置換は打ち切らず全件を対象にする）。
const BUFFER_SEARCH_MAX: usize = 20_000;

/// ⌘F バッファ内検索/置換バーの状態（アクティブタブのエディタに対して働く・M10）。
/// マッチ列はエディタ毎の状態なので、タブ/プロジェクト切替では畳む（持ち越さない）。
pub(crate) struct BufferSearchState {
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
pub(crate) struct CompletionState {
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
pub(crate) struct InlineEditState {
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
        .map(|line| {
            if line.starts_with("@@") {
                "···".to_string()
            } else {
                line.to_string()
            }
        })
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
pub(crate) struct CodeActionsState {
    /// (タイトル, アクションの生 JSON)。
    items: Vec<(SharedString, serde_json::Value)>,
    selected: usize,
    position: Point<gpui::Pixels>,
    focus: FocusHandle,
}

/// hover ポップアップ（LSP `textDocument/hover` の結果・M10）。フォーカスは取らない。
pub(crate) struct HoverState {
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
        Some(2) | Some(3) => "fn",    // Method / Function
        Some(5) => "field",           // Field
        Some(6) => "let",             // Variable
        Some(7) | Some(22) => "type", // Class / Struct
        Some(8) => "trait",           // Interface
        Some(9) => "mod",             // Module
        Some(14) => "kw",             // Keyword
        Some(21) => "const",          // Constant
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
            Some(CompletionItem {
                label: SharedString::from(label),
                insert_text,
                detail,
                kind,
            })
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
            location
                .pointer("/range/start/line")
                .and_then(|v| v.as_u64()),
            location
                .pointer("/range/start/character")
                .and_then(|v| v.as_u64()),
            location
                .pointer("/range/end/character")
                .and_then(|v| v.as_u64()),
        ) else {
            continue;
        };
        match by_file.iter_mut().find(|(existing, _)| existing == &path) {
            Some((_, hits)) => hits.push((line as u32, start_char as u32, end_char as u32)),
            None => by_file.push((
                path,
                vec![(line as u32, start_char as u32, end_char as u32)],
            )),
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

/// `.necoder/settings.json` の `color`（"#rrggbb"）と `icon`（絵文字）を読む（M12-11）。
/// 無ければ (None, None)。パース失敗は無視して既定へ。
fn read_project_identity(root: &Path) -> (Option<Hsla>, Option<SharedString>) {
    let path = root.join(".necoder/settings.json");
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

/// 通常の rail 切替で変更してよいのは active index だけ。
/// 同じ index と範囲外は no-op とし、session 配列には触れない。
fn active_index_after_switch(
    active: usize,
    requested: usize,
    project_count: usize,
) -> Option<usize> {
    (requested < project_count && requested != active).then_some(requested)
}

// ── Workspace 本体 ──

/// worktree / TaskSpace の安定 ID。レール添字や画面上のセル位置を永続 ID に使わない。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpaceId(String);

impl SpaceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn for_worktree(worktree: &Worktree) -> Self {
        Self(project::stable_worktree_id_on(
            worktree.host().as_ref(),
            worktree.root(),
        ))
    }
}

/// Task lifecycle と space 種別の単一定義は storage crate（FLEET-CONTROL-PLAN P0）。
/// GUI / CLI / MCP が同じ enum を共有し、ここでは再輸出だけする。
pub use storage::{SpaceKind, TaskPhase};

/// 1 worktree に紐づく Fleet の作業単位。通常 View では単なる project session として使え、
/// Fleet では task / review / integration の状態を持つ。
#[derive(Clone, Debug)]
pub struct TaskSpace {
    pub id: SpaceId,
    pub repository_id: String,
    pub title: SharedString,
    /// Integration（統合先の本流）か Task（隔離 worktree）か。phase とは別軸。
    pub kind: SpaceKind,
    pub phase: TaskPhase,
    pub base_oid: Option<String>,
    pub head_oid: Option<String>,
    pub result_summary: Option<SharedString>,
    pub created_at_ms: i64,
}

impl TaskSpace {
    fn is_integration(&self) -> bool {
        self.kind == SpaceKind::Integration
    }

    fn for_worktree(worktree: &Worktree, branch: Option<&str>) -> Self {
        let repository_id = project::repository_id_on(worktree.host().as_ref(), worktree.root());
        let head = project::git_head_oid_on(worktree.host().as_ref(), worktree.root());
        let detected_branch = branch.map(str::to_string).or_else(|| {
            project::git_current_branch_on(worktree.host().as_ref(), worktree.root())
                .filter(|branch| branch.starts_with("task/"))
        });
        let is_task = detected_branch.is_some();
        let title = detected_branch
            .as_deref()
            .map(|branch| {
                let title = branch
                    .strip_prefix("task/")
                    .unwrap_or(branch)
                    .replace(['-', '_'], " ");
                if title.chars().all(|character| character.is_ascii_digit()) {
                    format!("Task {title}")
                } else {
                    title
                }
            })
            .unwrap_or_else(|| worktree.name());
        Self {
            id: SpaceId::for_worktree(worktree),
            repository_id,
            title: SharedString::from(title),
            kind: if is_task {
                SpaceKind::Task
            } else {
                SpaceKind::Integration
            },
            phase: TaskPhase::Planned,
            base_oid: is_task.then(|| head.clone()).flatten(),
            head_oid: head,
            result_summary: None,
            created_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0),
        }
    }

    fn to_record(&self, slot: &ProjectSlot) -> storage::TaskSpaceRecord {
        storage::TaskSpaceRecord {
            id: self.id.0.clone(),
            repository_id: self.repository_id.clone(),
            root: slot.worktree.root().to_path_buf(),
            branch: slot.branch.clone().or_else(|| slot.worktree_branch.clone()),
            title: self.title.to_string(),
            kind: self.kind,
            phase: self.phase,
            base_oid: self.base_oid.clone(),
            head_oid: self.head_oid.clone(),
            result_summary: self.result_summary.as_ref().map(ToString::to_string),
            depends_on: Vec::new(), // 依存の正は台帳（task_deps）。GUI メモリ側は持たない（P6）
            created_at: self.created_at_ms,
            updated_at: self.created_at_ms,
        }
    }
}

/// 編隊グリッドの 1 surface。Task セルは worktree ごとの完整 AgentPanel を表示する。
/// 同じ Task へ Agent を追加するとセルを増やさず、パネル内の thread tab が増える。
/// Editor/Diff/Tests は 2026-07-24 に ＋ タイルの入口から外した（＋ = エージェント 1 択へ単純化）。
/// 機能は残す（後で控えめな入口から再接続する）ため dead_code を許す。
#[derive(Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) enum FleetPane {
    Task { space: SpaceId },
    Terminal { space: SpaceId },
    Editor { space: SpaceId },
    Diff { space: SpaceId },
    Tests { space: SpaceId },
}

/// 系譜グラフの表示（M14 #4）。扇形＝base から放射状に分岐 / ツリー＝上から下へ枝分かれ /
/// カード＝base→曲線→読めるカード / ハブ＝中央=リポジトリのハブ&スポーク。ヘッダのスイッチャーで切替。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GraphView {
    Fan,
    Tree,
    Card,
    Hub,
}

/// 編隊中央面のタブ（FLEET-CONTROL-PLAN P3）。Graph=系譜グラフ+グリッド / Control=管制ダッシュボード。
/// 既定は当面 Graph（ドッグフーディング後に再判断・計画 §P3）。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FleetCenterView {
    Graph,
    Control,
}

/// 編隊下段のタブ（2026-07-27）。ニュース＝task_events の鏡 / ターミナル＝アクティブ Task の実シェル。
/// 「ニュースは良いが、そこでターミナルも開きたい」というユーザー要望に対する最小の形 —
/// 面を増やさず、同じ 1 枚の高さを 2 つの用途で共有する（VSCode の下ドックと同じ考え方）。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FleetBottomView {
    News,
    Terminal,
}

/// Task（worktree）の表示名をダブルクリックで改名している最中の入力欄。
/// **どこで編集中か**（herd 見出し / セルヘッダ）を持つのは、編隊モードでは herd サイドバーと
/// セルグリッドが同時に見えており、同じ入力欄 Entity を 2 箇所で描くと二重描画になるため。
/// `index` は改名対象の project（レール slot）index。改名は表示名（`TaskSpace.title`）だけを変え、
/// git ブランチ `task/<n>` や worktree フォルダはそのまま（表示と git 識別を分離）。
pub(crate) struct TaskRenaming {
    pub(crate) index: usize,
    pub(crate) site: RenameSite,
    pub(crate) editor: Entity<EditorView>,
}

/// 改名入力欄をどこに描くか。同じ入力欄 Entity の二重描画を避ける識別に使う。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenameSite {
    /// herd（Fleet）サイドバーの Task 見出し。
    Herd,
    /// 編隊グリッドのセルヘッダのタイトル。
    Cell,
}

struct ChromeState {
    show_left: bool,
    show_right: bool,
    show_bottom: bool,
    show_settings: bool,
    /// 左ドックを herd サイドバー（編隊・状態一覧・M14）に切替中か。todo/git と排他（explorer が既定）。
    show_herd: bool,
    /// 編隊モード（全画面 = herd + 系譜グラフ + グリッド + ニュース・M14）。通常ビューを置き換える。
    fleet_mode: bool,
    /// 編隊グリッドのセル（mock の `.acell`・＋/× で増減・上限 8・M14 #3）。
    fleet_cells: Vec<FleetPane>,
    /// lanes からの自動配置を**もう済ませたか**（2026-07-27）。旧実装は「空なら seed」だったので、
    /// 最後の 1 枚を × で閉じた次のフレームに全セルが復活していた（ユーザー報告「罰おしても消えない」）。
    /// 空はユーザーが選んだ状態でもありうる ＝ 空かどうかで判断してはいけない。
    fleet_seeded: bool,
    /// 編隊中央のタブ（管制 / グラフ・P3）。
    fleet_center_view: FleetCenterView,
    /// 管制タブのフォーカス（⏎ = キュー先頭へ・keymap context "FleetControl" の足場）。
    control_focus: FocusHandle,
    /// 編隊モードの herd で solo（Integration リポジトリ）のスレッド群を展開しているか。
    /// **既定 false = 畳む**（編隊では Task が主役・2026-07-24 ユーザー指摘）。見出しクリックで切替。
    herd_solo_expanded: bool,
    /// Task の表示名をダブルクリックで改名中（herd 見出し / セルヘッダ）。スレッドタブの改名と同型。
    task_renaming: Option<TaskRenaming>,
    /// 系譜グラフの表示（扇形/リバー/ツリー/カード・M14 #4）。
    graph_view: GraphView,
    /// 系譜グラフを畳んでいるか（⌄・ヘッダのみ表示）。
    graph_collapsed: bool,
    /// 拡大表示中のセル（mock の focus・M14）。Some のとき系譜グラフを隠し、そのセルを大きく・
    /// 他セルはサムネイル列に避ける。index は `fleet_cells` の添字（範囲外は None 扱い）。
    fleet_maximized: Option<usize>,
    /// 編隊セルの ⋯ メニュー（片付けの 4 段を 1 枚に並べる・2026-07-27）。
    fleet_cell_menu: Option<FleetCellMenuState>,
    /// 編隊下段のタブ（ニュース / ターミナル・2026-07-27）。
    fleet_bottom_view: FleetBottomView,
    /// solo で AI パネルだけを全画面にしているか（2026-07-27）。レールは残す（プロジェクト切替と
    /// 復帰の導線を失わないため）。エディタ列・エクスプローラ・下ドックは畳む。
    agent_full_screen: bool,
    /// 下段の高さ（px）。上縁ドラッグで変わる。ニュース/ターミナルで共有する（同じ 1 枚だから）。
    bottom_height: f32,
    resizing_bottom: bool,
    resize_start_y: f32,
    resize_start_height: f32,
    /// 相対時刻（開始/最終入力の「N分前」）を編隊・herd 表示中だけ更新する 30 秒時計が稼働中か
    /// （多重起動防止。両方閉じたら次 tick で自停止＝idle 予算を守る・M14）。
    fleet_clock: bool,
    /// フッターのニュース欄（稼働ロールアップ）で今見せているシグナルの番号。2件以上ある間だけ
    /// タイマーで進めて順に切り替える（複数の実行中がちゃんと全部見えるように・2026-08-04）。
    rollup_index: usize,
    /// ニュース欄の回転タイマーが稼働中か（多重起動防止。2件未満で次 tick 自停止＝idle 予算を守る）。
    rollup_ticker: bool,
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
    /// レール項目のドラッグ状態（index・押下位置・閾値超えフラグ）。窓の外で離すと
    /// 擬似 tear-off = その位置に新窓（M13。本物の tear-off は gpui 未対応・DECISIONS）。
    rail_drag: Option<(usize, Point<gpui::Pixels>, bool)>,
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
    /// worktree 削除の確認ダイアログ（2026-07-27）。何を失うかを git に聞いて見せる。
    worktree_delete: Option<worktree_delete::WorktreeDeleteConfirm>,
    ssh_input: Option<(String, FocusHandle)>,
    ssh_connecting: bool,
    add_project_dialog_open: bool,
    pending_project_switch: Option<usize>,
}

struct NotificationCenter {
    /// (本文, 色, 世代番号, ジャンプ先)。ジャンプ先 = `Some((session_index, thread_index))` の時、
    /// クリックでそのプロジェクト＋スレッドへ切り替える（権限待ちトースト用・それ以外は None）。
    toasts: Vec<(SharedString, Hsla, u32, Option<(usize, usize)>)>,
    toast_gen: u32,
    /// 前回クラッシュのログパス（起動時に pending マーカーから 1 回だけ拾う・M13）。
    /// Some の間 statusbar に ⚠ チップ → クリックでバグ報告 Issue を開いて消える。
    crash_notice: Option<PathBuf>,
    /// ニュースフィード（管制 P2）: `task_events` と同型の時系列ログ（新しいものが先頭）。
    /// 起動時に DB から backfill し、以後は task_events へ書くのと同じ場所で live に積む。
    /// **将来の監督（coordinator）の采配も同じ形でここへ載る**（監査可能なニュース）。
    news: Vec<NewsItem>,
}

/// ニュース 1 行（mock `fleet-dashboard.html` 下段の書式）: 時刻 + 帰属チップ + **太字名** + イベント文。
#[derive(Clone)]
pub(crate) struct NewsItem {
    pub at_ms: i64,
    /// 帰属チップの色（スレッド色 / TaskSpace 色）。**色は識別・種別は形**（coordinator は丸チップ）。
    pub color: Hsla,
    /// 太字部（タスク名 / スレッド名 / 「監督」）。
    pub title: SharedString,
    pub text: SharedString,
    pub kind: NewsKind,
}

/// ニュースのイベント種別（task_events の kind と同じ語彙・監督の采配も同じログに載る前提の設計）。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum NewsKind {
    PhaseChange,
    Permission,
    Digest,
    Integration,
    #[allow(dead_code)] // P6（監督席）が載せる。ニュースの語彙として先に確保する。
    Coordinator,
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
    /// 管制バーの固定サイズマスコット。atlas時計はWorkspace全体をinvalidにしない。
    fleet_mascot: Entity<agent_panel::MascotView>,
    /// この窓がアクティブか（render で更新）。管制のマスコット等が「動き」を止める判定に使う。
    window_active: bool,
    /// 経過秒・承認待ち表示だけの1Hz時計。マスコットの5/10fps時計は子Entityに分離済み。
    visual_tick: u64,
    visual_ticker: bool,
    /// 編隊レベルの ✳ 総括（Tier 2・P4）。キューに影響する遷移から 5s デバウンスで oneshot 生成。
    /// **状態を上書きしない**（数字とキューは事実層・これは監督バーに添える文）。
    control_summary: Option<SharedString>,
    /// 総括のデバウンス世代（最新の遷移だけが生成を走らせる）。
    control_summary_gen: u32,
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

/// Task 名がプレースホルダ（"Task" / "Task <数字>"）のままか。AI 命名の引き継ぎ判定（2026-07-24）。
pub(crate) fn is_placeholder_task_title(title: &str) -> bool {
    title == "Task"
        || title
            .strip_prefix("Task ")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

/// パスの末尾 `max` 階層のフォルダ名（ルート `/` は除く）。エクスプローラの上位階層ブレッドクラム用。
/// titlebar beacon のドット。状態の反復アニメーションはマスコットへ一本化し、ここは静止表示。
fn beacon_dot(_id: impl Into<gpui::ElementId>, color: Hsla, running: bool) -> gpui::AnyElement {
    let base = if running { color } else { color.alpha(0.5) };
    div()
        .size(px(8.))
        .rounded(px(4.))
        .flex_none()
        .bg(base)
        .into_any_element()
}

/// スレッド状態 → 表示ラベル（beacon / フッターロールアップ / ⌘O / herd で共用・i18n）。
pub(crate) fn activity_label(activity: agent_panel::ThreadActivity) -> SharedString {
    use agent_panel::ThreadActivity;
    match activity {
        ThreadActivity::Working => i18n::t!("agent.state_working").into(),
        ThreadActivity::Blocked => i18n::t!("agent.state_blocked").into(),
        ThreadActivity::Done { interrupted: false } => i18n::t!("agent.state_done").into(),
        ThreadActivity::Done { interrupted: true } => {
            i18n::t!("agent.state_done_interrupted").into()
        }
        ThreadActivity::Idle => i18n::t!("agent.state_idle").into(),
    }
}

/// 「開始 N分前 · 入力 N分前」の 1 行（herd 行 / 編隊セル状態行で共用・M14）。
/// いつスタートして最終いつ入力したかをサクッと見せる。入力がまだ無ければ「開始 …」だけ。
pub(crate) fn thread_times_label(
    created_at_ms: i64,
    last_input_at_ms: Option<i64>,
) -> SharedString {
    let started =
        i18n::t!("time.started", "when" => agent_panel::relative_time_label(created_at_ms));
    match last_input_at_ms {
        Some(last_input_at_ms) => SharedString::from(format!(
            "{started} · {}",
            i18n::t!("time.last_input", "when" => agent_panel::relative_time_label(last_input_at_ms))
        )),
        None => SharedString::from(started),
    }
}

/// 拡張子別のアイコン色（識別用。theme の syntax パレットを流用＝色による方向感覚）。
fn file_type_color(name: &str, theme: &Theme) -> Hsla {
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let syntax = &theme.syntax;
    match lang::language_for_name(name) {
        Some(lang::LanguageId::Rust) => syntax.number,
        Some(lang::LanguageId::Toml | lang::LanguageId::Yaml | lang::LanguageId::Json) => {
            syntax.function
        }
        Some(lang::LanguageId::Markdown) => theme.fg2,
        Some(
            lang::LanguageId::TypeScript | lang::LanguageId::Tsx | lang::LanguageId::JavaScript,
        ) => syntax.type_,
        Some(lang::LanguageId::Python | lang::LanguageId::Go | lang::LanguageId::Bash) => {
            syntax.string
        }
        Some(lang::LanguageId::Html | lang::LanguageId::Css) => syntax.keyword,
        Some(lang::LanguageId::C | lang::LanguageId::Cpp) => syntax.type_,
        None if matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico"
        ) =>
        {
            syntax.macro_
        }
        None if matches!(extension.as_str(), "txt" | "log") => theme.fg2,
        None => theme.fg1,
    }
}

/// ファイル名（とディレクトリか否か）から SVG アイコンのパスを引く。言語ロゴ = Simple Icons（CC0）、
/// フォルダ/汎用 = Lucide（ISC）。拡張子判定は `lang` の言語表に寄せ、名前でしか判らないもの
/// （Dockerfile・.git*）だけ先に拾う。未知の拡張子は汎用ファイルにフォールバック。
fn file_icon_path(name: &str, is_dir: bool, is_expanded: bool) -> &'static str {
    if is_dir {
        return if is_expanded {
            "icons/folder-open.svg"
        } else {
            "icons/folder.svg"
        };
    }
    let lower = name.to_ascii_lowercase();
    if lower == "dockerfile" || lower.starts_with("dockerfile.") || lower.ends_with(".dockerfile") {
        return "icons/file-docker.svg";
    }
    if lower.starts_with(".git") {
        // .gitignore / .gitattributes / .gitmodules など（拡張子を持たない git 系）。
        return "icons/file-git.svg";
    }
    let extension = lower.rsplit('.').next().unwrap_or("");
    match lang::language_for_name(name) {
        Some(lang::LanguageId::Rust) => "icons/file-rust.svg",
        Some(lang::LanguageId::JavaScript) => "icons/file-javascript.svg",
        Some(lang::LanguageId::TypeScript) => "icons/file-typescript.svg",
        Some(lang::LanguageId::Tsx) => "icons/file-tsx.svg",
        Some(lang::LanguageId::Python) => "icons/file-python.svg",
        Some(lang::LanguageId::Go) => "icons/file-go.svg",
        Some(lang::LanguageId::Json) => "icons/file-json.svg",
        Some(lang::LanguageId::Yaml) => "icons/file-yaml.svg",
        Some(lang::LanguageId::Toml) => "icons/file-toml.svg",
        Some(lang::LanguageId::Html) => "icons/file-html.svg",
        Some(lang::LanguageId::Css) => "icons/file-css.svg",
        Some(lang::LanguageId::Markdown) => "icons/file-markdown.svg",
        Some(lang::LanguageId::Bash) => "icons/file-shell.svg",
        Some(lang::LanguageId::C) => "icons/file-c.svg",
        Some(lang::LanguageId::Cpp) => "icons/file-cpp.svg",
        None if matches!(
            extension,
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "avif"
        ) =>
        {
            "icons/file-image.svg"
        }
        None if matches!(extension, "txt" | "log" | "text") => "icons/file-text.svg",
        None => "icons/file-generic.svg",
    }
}

/// エクスプローラのファイル/フォルダアイコン（SVG・単色マスク）。形で型を見分け、色は
/// `file_type_color` で薄く付ける（フォルダは folder 色）。固定幅スロットに入れて名前を揃える。
fn file_icon(name: &str, is_dir: bool, is_expanded: bool, theme: &Theme) -> impl IntoElement {
    let color = if is_dir {
        theme.folder_icon()
    } else {
        file_type_color(name, theme)
    };
    div()
        .flex_none()
        .w(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(file_icon_path(name, is_dir, is_expanded))
                .size(px(14.))
                .text_color(color),
        )
}

/// アイコングリッド用の大きめアイコン（[`file_icon`] の約 2 倍）。
fn icon_large(name: &str, is_dir: bool, theme: &Theme) -> impl IntoElement {
    let color = if is_dir {
        theme.folder_icon()
    } else {
        file_type_color(name, theme)
    };
    div().flex().items_center().justify_center().child(
        svg()
            .path(file_icon_path(name, is_dir, false))
            .size(px(30.))
            .text_color(color),
    )
}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.active_editor() {
            Some(editor) => editor.read(cx).focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl Workspace {
    /// 管制マスコットの 10fps と、承認待ちの低頻度 pulse（単独時2fps）を束ねる時計。
    /// 対象が見えない時や窓の非アクティブ化で次の tick に自停止する。
    fn ensure_visual_ticker(&mut self, cx: &mut Context<Self>) {
        if self.visual_ticker {
            return;
        }
        self.visual_ticker = true;
        cx.spawn(async move |workspace, cx| loop {
            let next_delay = workspace.update(cx, |workspace, cx| {
                let control_visible = workspace.chrome.fleet_mode
                    && workspace.chrome.fleet_center_view == FleetCenterView::Control;
                let attention_visible = workspace.waiting_thread.is_some();
                if !workspace.window_active || (!control_visible && !attention_visible) {
                    workspace.visual_ticker = false;
                    return None;
                }
                workspace.visual_tick = workspace.visual_tick.wrapping_add(1);
                cx.notify();
                Some(std::time::Duration::from_secs(1))
            });
            let Ok(Some(delay)) = next_delay else {
                break;
            };
            cx.background_executor().timer(delay).await;
        })
        .detach();
    }

    /// Window を必要とする child event の後処理が残っているか。
    ///
    /// child の subscription は `Window` を受け取らないため、描画中に実行せず、現在の
    /// effect cycle の末尾へまとめて送る。これにより root Render は状態変更や I/O を行わない。
    fn has_pending_shell_effects(&self) -> bool {
        self.chrome.pending_settings_command.is_some()
            || self.pending_transient_tab.is_some()
            || self.pending_open_history
            || self.overlays.pending_project_switch.is_some()
            || self.pending_navigation.is_some()
            || self.pending_open_git_diff.is_some()
            || self.pending_stage_hunk.is_some()
    }

    fn process_pending_shell_effects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = self.chrome.pending_settings_command.take() {
            self.open_command_terminal(&command, window, cx);
        }
        if let Some((title, buffer)) = self.pending_transient_tab.take() {
            self.open_transient_tab(title, buffer, window, cx);
        }
        if self.pending_open_history {
            self.pending_open_history = false;
            self.open_thread_history(&ThreadHistory, window, cx);
        }
        if let Some(index) = self.overlays.pending_project_switch.take() {
            self.switch_project(index, window, cx);
        }
        if let Some((path, line, column)) = self.pending_navigation.take() {
            self.record_nav_position(cx);
            self.open_file_then(path, window, cx, move |editor, cx| {
                editor.reveal_position(line, column, cx);
            });
        }
        if let Some(path) = self.pending_open_git_diff.take() {
            self.open_diff_tab_for(path, None, window, cx);
        }
        if let Some(hunk) = self.pending_stage_hunk.take() {
            self.stage_hunk(hunk, cx);
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.window_active = window.is_window_active(); // 管制マスコット等の「動き」判定（P3）
        let control_mascot_visible =
            self.chrome.fleet_mode && self.chrome.fleet_center_view == FleetCenterView::Control;
        if self.window_active && (control_mascot_visible || self.waiting_thread.is_some()) {
            self.ensure_visual_ticker(cx);
        }
        if self.window_active && (self.chrome.fleet_mode || self.chrome.show_herd) {
            self.ensure_fleet_clock(cx);
        }
        self.ensure_rollup_ticker(cx); // フッターのニュース欄を複数稼働時に順送りする（自停止）

        if self.has_pending_shell_effects() {
            let workspace = cx.entity();
            window.defer(cx, move |window, cx| {
                let _ = workspace.update(cx, |workspace, cx| {
                    workspace.process_pending_shell_effects(window, cx);
                });
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
            .on_action(cx.listener(Self::toggle_herd_sidebar))
            .on_action(cx.listener(Self::toggle_fleet_mode))
            .on_action(cx.listener(Self::toggle_agent_full_screen))
            .on_action(cx.listener(Self::toggle_control_center))
            .on_action(cx.listener(Self::control_next))
            .on_action(cx.listener(Self::open_command_palette))
            .on_action(cx.listener(Self::report_bug_action))
            .on_action(cx.listener(Self::open_settings_action))
            .on_action(cx.listener(Self::about_action))
            .on_action(cx.listener(Self::open_recent_action))
            .on_action(cx.listener(Self::open_dialog_action))
            // macOS 標準のアプリ/ウィンドウ操作（メニューバー・M13）。cx は App へ deref。
            .on_action(cx.listener(|_, _: &Hide, _window, cx| cx.hide()))
            .on_action(cx.listener(|_, _: &HideOthers, _window, cx| cx.hide_other_apps()))
            .on_action(cx.listener(|_, _: &ShowAll, _window, cx| cx.unhide_other_apps()))
            .on_action(cx.listener(|_, _: &Minimize, window, _| window.minimize_window()))
            .on_action(cx.listener(|_, _: &Zoom, window, _| window.zoom_window()))
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
            .on_action(cx.listener(|this, _: &ActivateProject1, window, cx| {
                this.switch_project(0, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateProject2, window, cx| {
                this.switch_project(1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateProject3, window, cx| {
                this.switch_project(2, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateProject4, window, cx| {
                this.switch_project(3, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateProject5, window, cx| {
                this.switch_project(4, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateProject6, window, cx| {
                this.switch_project(5, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateProject7, window, cx| {
                this.switch_project(6, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateProject8, window, cx| {
                this.switch_project(7, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateProject9, window, cx| {
                this.switch_project(8, window, cx)
            }))
            // レール上下への循環切替（⌃⌘↑↓）。番号（⌘1..9）を覚えずに隣へ流せる。
            .on_action(cx.listener(|this, _: &NextProject, window, cx| {
                let count = this.project_sessions.projects.len();
                if count > 1 {
                    let next = (this.project_sessions.active + 1) % count;
                    this.switch_project(next, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PrevProject, window, cx| {
                let count = this.project_sessions.projects.len();
                if count > 1 {
                    let previous = (this.project_sessions.active + count - 1) % count;
                    this.switch_project(previous, window, cx);
                }
            }))
            .on_mouse_move(cx.listener(Self::on_resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_resize_end))
            // レール項目の擬似 tear-off（枠外ドロップ → 新窓・M13）。resize と独立のリスナー。
            .on_mouse_move(cx.listener(Self::on_rail_drag_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_rail_drag_end))
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
            .child(if self.chrome.fleet_mode {
                // 編隊モード（mock の「編隊」ビュー・M14）: 通常の center/right dock を丸ごと置換。
                if !self.chrome.fleet_seeded {
                    self.seed_fleet_cells(cx); // 初回（起動プローブ含む）は lanes で自動配置
                                               // 開発用: NECODER_FLEET_ADD=n で ＋Agent を n 回（新スレッド起動が複製しない検証）。
                    if let Ok(n) = std::env::var("NECODER_FLEET_ADD")
                        .unwrap_or_default()
                        .parse::<usize>()
                    {
                        for _ in 0..n {
                            self.add_fleet_agent(cx);
                        }
                    }
                }
                self.render_fleet(cx)
            } else {
                // 通常レイアウト。**AI 全画面（`agent_full_screen`）は中央のエディタを Agent に差し替える**
                // だけの面へ変更（2026-08-08 本人要望）。左ドック（ファイルブラウザ）と下ドック（ターミナル）は
                // 各自の ON/OFF に従わせ、冗長になる右の Agent ドックだけ畳む＝「全画面が全部を消す」のをやめる。
                // 純チャット（ファイルを見ない）にしたければ左ドックを OFF にすれば良い（明示操作に寄せる）。
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .when(self.chrome.show_left, |element| {
                        // 左カラムは herd / Todo ボード / git パネル / エクスプローラを切替（排他）。
                        let column = if self.chrome.show_herd {
                            self.render_herd_sidebar(cx)
                        } else if self.todo_panel.read(cx).open {
                            self.render_todo_board(cx)
                        } else if self.git_panel_open(cx) {
                            self.render_git_panel(cx)
                        } else {
                            self.render_explorer(cx).into_any_element()
                        };
                        element.child(column)
                    })
                    .child(if self.chrome.agent_full_screen {
                        self.render_agent_full_center(cx).into_any_element()
                    } else {
                        self.render_center(cx).into_any_element()
                    })
                    .when(
                        self.chrome.show_right && !self.chrome.agent_full_screen,
                        |element| element.child(self.render_agent_dock(cx)),
                    )
                    .into_any_element()
            })
            .child(self.render_statusbar(cx))
            // オーバーレイ（最前面）
            .when_some(self.overlays.picker.clone(), |this, picker| {
                this.child(picker)
            })
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
            .children(self.render_fleet_cell_menu(cx))
            .children(self.render_worktree_delete_dialog(cx))
            .children(self.render_branch_menu(cx))
            .children(self.render_explorer_context_menu(cx))
            .children(self.render_confetti(cx)) // 最前面（祝いの紙吹雪）
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Render 中の Host trait 呼び出しを local / remote の両方で検出する wrapper。
    struct RenderAuditHost {
        inner: Arc<dyn Host>,
        remote: bool,
        armed: AtomicBool,
        calls: AtomicUsize,
    }

    impl RenderAuditHost {
        fn new(remote: bool) -> Self {
            Self {
                inner: host::LocalHost::shared(),
                remote,
                armed: AtomicBool::new(false),
                calls: AtomicUsize::new(0),
            }
        }

        fn arm(&self) {
            self.calls.store(0, Ordering::SeqCst);
            self.armed.store(true, Ordering::SeqCst);
        }

        fn disarm(&self) {
            self.armed.store(false, Ordering::SeqCst);
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn record(&self) {
            if self.armed.load(Ordering::SeqCst) {
                self.calls.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    impl Host for RenderAuditHost {
        fn id(&self) -> &str {
            self.record();
            if self.remote {
                "render-audit-remote"
            } else {
                "render-audit-local"
            }
        }

        fn display_name(&self) -> &str {
            self.record();
            if self.remote {
                "Render Audit Remote"
            } else {
                "Render Audit Local"
            }
        }

        fn is_remote(&self) -> bool {
            self.record();
            self.remote
        }

        fn project_uri(&self, path: &Path) -> Option<String> {
            self.record();
            self.remote
                .then(|| format!("ssh://render-audit{}", path.display()))
        }

        fn host_for_project(&self, path: &Path) -> anyhow::Result<Arc<dyn Host>> {
            self.record();
            self.inner.host_for_project(path)
        }

        fn canonicalize(&self, path: &Path) -> anyhow::Result<PathBuf> {
            self.record();
            self.inner.canonicalize(path)
        }

        fn metadata(&self, path: &Path) -> anyhow::Result<host::HostMetadata> {
            self.record();
            self.inner.metadata(path)
        }

        fn read_dir(&self, path: &Path) -> anyhow::Result<Vec<host::HostEntry>> {
            self.record();
            self.inner.read_dir(path)
        }

        fn read_file(&self, path: &Path) -> anyhow::Result<host::FileContent> {
            self.record();
            self.inner.read_file(path)
        }

        fn write_file(
            &self,
            path: &Path,
            bytes: &[u8],
            condition: host::WriteCondition,
        ) -> anyhow::Result<host::FileRevision> {
            self.record();
            self.inner.write_file(path, bytes, condition)
        }

        fn list_files(&self, root: &Path, limit: usize) -> anyhow::Result<Vec<PathBuf>> {
            self.record();
            self.inner.list_files(root, limit)
        }

        fn search_project(
            &self,
            root: &Path,
            spec: &host::TextSearchSpec,
            file_limit: usize,
        ) -> anyhow::Result<Vec<host::TextSearchHit>> {
            self.record();
            self.inner.search_project(root, spec, file_limit)
        }

        fn run_command(&self, spec: &host::CommandSpec) -> anyhow::Result<host::CommandOutput> {
            self.record();
            self.inner.run_command(spec)
        }

        fn spawn_process(&self, spec: &host::CommandSpec) -> anyhow::Result<host::HostProcess> {
            self.record();
            self.inner.spawn_process(spec)
        }

        fn terminal_launch(&self, cwd: &Path) -> anyhow::Result<Option<host::TerminalLaunch>> {
            self.record();
            self.inner.terminal_launch(cwd)
        }
    }

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
    fn project_switch_only_selects_an_existing_different_session() {
        assert_eq!(active_index_after_switch(0, 1, 2), Some(1));
        assert_eq!(active_index_after_switch(1, 0, 2), Some(0));
        assert_eq!(active_index_after_switch(1, 1, 2), None);
        assert_eq!(active_index_after_switch(0, 2, 2), None);
    }

    #[gpui::test]
    fn project_switch_preserves_dirty_undo_and_child_entities(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_workspace_switch_{}", std::process::id()));
        let project_a = root.join("a");
        let project_b = root.join("b");
        let file_a = project_a.join("a.txt");
        let file_b = project_b.join("b.txt");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();
        std::fs::write(&file_a, "fn a() {}\n").unwrap();
        std::fs::write(&file_b, "fn b() {}\n").unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(&settings_path, r#"{"onboarded":true}"#).unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));

        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(
                vec![project_a.clone(), project_b.clone()],
                Theme::dark(),
                None,
                cx,
            )
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.restore_open_file(
                &[
                    RestoredTabs::single(file_a.clone()),
                    RestoredTabs::single(file_b.clone()),
                ],
                window,
                cx,
            );
        });

        let editor_a = workspace.read_with(cx, |workspace, _cx| {
            workspace.active_editor().expect("project A editor")
        });
        editor_a.update(cx, |editor, cx| editor.insert_text("dirty_", cx));
        assert!(editor_a.read_with(cx, |editor, _| editor.buffer().is_dirty()));
        assert!(editor_a.read_with(cx, |editor, _| editor.buffer().text().starts_with("dirty_")));

        let entities_a = workspace.update_in(cx, |workspace, _window, cx| {
            let session = &workspace.project_sessions.sessions[0];
            let terminal_dock = session.terminal_dock.clone();
            let terminal = terminal_dock.update(cx, |dock, cx| dock.ensure_active_test(cx));
            (
                session.agent_panel.entity_id(),
                session.explorer.entity_id(),
                session.git_panel.entity_id(),
                session.todo_panel.entity_id(),
                session.tests_dock.entity_id(),
                terminal_dock.entity_id(),
                terminal.entity_id(),
            )
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.switch_project(1, window, cx);
        });
        let entities_b = workspace.update_in(cx, |workspace, _window, cx| {
            let session = &workspace.project_sessions.sessions[1];
            let terminal_dock = session.terminal_dock.clone();
            let terminal = terminal_dock.update(cx, |dock, cx| dock.ensure_active_test(cx));
            (
                session.agent_panel.entity_id(),
                session.explorer.entity_id(),
                session.git_panel.entity_id(),
                session.todo_panel.entity_id(),
                session.tests_dock.entity_id(),
                terminal_dock.entity_id(),
                terminal.entity_id(),
            )
        });
        assert_ne!(
            entities_a, entities_b,
            "projects must not share child entities"
        );

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.switch_project(0, window, cx);
        });
        let (returned_editor, returned_entities) =
            workspace.update_in(cx, |workspace, _window, cx| {
                let session = &workspace.project_sessions.sessions[0];
                let terminal_dock = session.terminal_dock.clone();
                let terminal = terminal_dock.update(cx, |dock, cx| dock.ensure_active_test(cx));
                (
                    workspace
                        .active_editor()
                        .expect("returned project A editor"),
                    (
                        session.agent_panel.entity_id(),
                        session.explorer.entity_id(),
                        session.git_panel.entity_id(),
                        session.todo_panel.entity_id(),
                        session.tests_dock.entity_id(),
                        terminal_dock.entity_id(),
                        terminal.entity_id(),
                    ),
                )
            });
        assert_eq!(returned_editor.entity_id(), editor_a.entity_id());
        assert_eq!(returned_entities, entities_a);
        assert!(
            returned_editor.read_with(cx, |editor, _| editor.buffer().text().starts_with("dirty_"))
        );

        let editor_focus = returned_editor.read_with(cx, |editor, cx| editor.focus_handle(cx));
        cx.update(|window, cx| {
            window.focus(&editor_focus, cx);
            editor_focus.dispatch_action(&editor_view::Undo, window, cx);
        });
        assert_eq!(
            returned_editor.read_with(cx, |editor, _| editor.buffer().text()),
            "fn a() {}\n",
            "undo history must survive A → B → A",
        );

        // 監視先を消す前に notify watcher を全セッション分落とす。監視中の root を
        // 削除したまま process teardown に入ると、fsevents スレッドのデストラクタが
        // panic → SIGABRT する race がある（フルスイート並列実行時のみ顕在化）。
        workspace.update_in(cx, |workspace, _window, _cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        let _ = std::fs::remove_dir_all(root);
    }

    #[gpui::test]
    fn local_and_remote_root_render_never_call_host(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "necoder_workspace_render_audit_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("audit.txt"), "render audit\n").unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(&settings_path, r#"{"onboarded":true}"#).unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));

        let local = Arc::new(RenderAuditHost::new(false));
        let remote = Arc::new(RenderAuditHost::new(true));
        let sources = vec![
            ProjectSource::new(local.clone(), root.clone()),
            ProjectSource::new(remote.clone(), root.clone()),
        ];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new_sources(sources, Theme::dark(), None, cx)
        });

        local.arm();
        remote.arm();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        local.disarm();
        remote.disarm();
        assert_eq!(local.calls(), 0, "local Render must use cached data only");
        assert_eq!(
            remote.calls(),
            0,
            "inactive remote session must not be queried"
        );

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.switch_project(1, window, cx);
        });
        cx.run_until_parked();
        local.arm();
        remote.arm();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        local.disarm();
        remote.disarm();
        assert_eq!(
            local.calls(),
            0,
            "inactive local session must not be queried"
        );
        assert_eq!(remote.calls(), 0, "remote Render must use cached data only");

        // 上のテストと同じ teardown race 回避（watcher を root 削除より先に落とす）。
        workspace.update_in(cx, |workspace, _window, _cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn inline_edit_diff_lines_marks_changes_and_truncates() {
        // 変更を +/- で表示（受入: 「その場に diff 表示」の中身・M12-8）。
        let old_text = "fn get() -> u32 {\n    42\n}\n";
        let new_text = "fn get() -> Result<u32, Error> {\n    Ok(42)\n}\n";
        let lines = inline_edit_diff_lines(old_text, new_text, 14);
        assert!(lines
            .iter()
            .any(|line| line.starts_with("-fn get() -> u32")));
        assert!(lines
            .iter()
            .any(|line| line.starts_with("+fn get() -> Result")));
        // ファイルヘッダ（---/+++）は出さない。@@ は ··· に置換。
        assert!(!lines
            .iter()
            .any(|line| line.starts_with("---") || line.starts_with("+++")));
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
        let dir = std::env::temp_dir().join(format!("necoder_refs_{}", std::process::id()));
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
