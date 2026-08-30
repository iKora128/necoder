//! editor_view — [`editor_core::Buffer`] を GPUI で描く編集ビュー。
//!
//! 公開 GPUI API と `gpui/examples/input.rs` を利用例として、necoder の Buffer と要件から組み立てた
//! 複数行版: 行仮想化・行番号ガター・キャレット・縦スクロール・キーボード/マウス・IME。
//! 色は theme_core、文字列は i18n（`t!`）経由（UI-SPEC の許可リスト厳守）。

use editor_core::{Buffer, BufferSnapshot, Point as BufferPoint, Selection};
use gpui::{
    actions, div, fill, hsla, point, prelude::*, px, relative, size, App, Bounds, Context,
    CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter,
    FocusHandle, Focusable, GlobalElementId, InspectorElementId, IntoElement, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ScrollWheelEvent, ShapedLine, SharedString, Style, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window,
};
use std::ops::Range;
use theme_core::{SyntaxColors, Theme};

/// .md の整形プレビュー（rendered 側の Block→GPUI 描画）。source ⇄ rendered は ⌘⇧V。
mod markdown_preview;

const FONT_SIZE: f32 = 13.0; // code 13px（既定。settings の font_size で上書き・M10-13）
const LINE_HEIGHT: f32 = 23.0; // compact 行高 23（font_size に比例して伸縮）
const GUTTER_PADDING: f32 = 10.0; // 行番号の左右余白
const GUTTER_MIN_WIDTH: f32 = 46.0; // UI-SPEC §1.4 行番号ガター 46
/// 既定タブ幅（空白換算）。settings の `tab_size` 配線（M10-13）までの暫定値。
const DEFAULT_TAB_SIZE: usize = 4;

/// クリック起点のドラッグ選択粒度（単=文字 / ダブル=単語 / トリプル以上=行。macOS 慣例）。
/// mouse down で決まり、mouse move の拡張も同じ粒度で行う（起点の単語/行を必ず含める）。
#[derive(Clone)]
enum DragSelectMode {
    Char,
    Word { origin: Range<usize> },
    Line { origin_row: usize },
}

actions!(
    editor,
    [
        Backspace,
        Delete,
        Newline,
        InsertNewline,
        // ── 編集の所作（M10-9） ──
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordBackward,
        MoveToStart,
        MoveToEnd,
        MoveLineUp,
        MoveLineDown,
        DuplicateLineUp,
        DuplicateLineDown,
        DeleteLine,
        ToggleComment,
        TabIndent,
        Indent,
        Outdent,
        // ── multi-cursor（M10-10） ──
        SelectNext,
        AddCursorAbove,
        AddCursorBelow,
        Cancel,
        // ── soft wrap（M10-12・⌥Z） ──
        ToggleSoftWrap,
        // ── markdown 整形プレビュー（⌘⇧V・.md のみ） ──
        ToggleRenderedMarkdown,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveToLineStart,
        MoveToLineEnd,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Save,
    ]
);

/// composer（平坦モード）が親（agent_panel）へ通知するイベント。
/// UI 非依存を保つため EditorView は「送信」を知らず、確定要求だけを emit する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerEvent {
    /// Enter 送信が有効かつ IME 変換中でない Enter が押された（＝親が送信すべき）。
    Submit,
}

/// キーボード入力の確定テキスト通知（補完の自動トリガ用・M10）。
/// **入力ハンドラ経由の確定入力のみ** emit する（IME 変換中・paste・undo・補完適用では出さない）。
/// workspace がタブ毎に subscribe し、識別子/`.`/`::` で補完を自動トリガする。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorInputEvent {
    /// 確定テキストが挿入された（通常タイプは 1 文字・IME 確定は複数文字）。
    Typed(String),
    /// クリックでキャレットが大きく飛んだ（ナビ履歴に積む・M10-11）。値は移動**前**の byte offset。
    CaretJumped { from: usize },
    /// gutter の diff バーがクリックされた（hunk 操作ポップオーバー・M11-10）。
    HunkClicked {
        hunk: project::DiffHunk,
        position: Point<Pixels>,
    },
}

/// マウスが同じ位置に ~500ms 留まった通知（LSP hover の配線用・M10）。
/// workspace が subscribe して `textDocument/hover` を要求する。
#[derive(Debug, Clone, PartialEq)]
pub enum EditorHoverEvent {
    /// 留まった位置。`line`/`character` は LSP 位置（UTF-16）・`anchor` はウィンドウ座標。
    Dwell {
        line: u32,
        character: u32,
        anchor: Point<Pixels>,
    },
    /// hover を消すべき操作（クリック・スクロール・dwell 位置から大きく離れた）。
    Cancel,
}

/// hover の dwell 判定時間。
const HOVER_DWELL_MS: u64 = 500;
/// dwell 位置からこの距離（Manhattan）を超えて動いたら hover を消す。
const HOVER_CANCEL_DISTANCE: f32 = 30.0;

/// 既定キーマップを登録する（bin から起動時に呼ぶ）。コンテキストは "Editor"。
pub fn bind_default_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Editor")),
        KeyBinding::new("delete", Delete, Some("Editor")),
        KeyBinding::new("enter", Newline, Some("Editor")),
        KeyBinding::new("shift-enter", InsertNewline, Some("Editor")),
        KeyBinding::new("left", MoveLeft, Some("Editor")),
        KeyBinding::new("right", MoveRight, Some("Editor")),
        KeyBinding::new("up", MoveUp, Some("Editor")),
        KeyBinding::new("down", MoveDown, Some("Editor")),
        KeyBinding::new("home", MoveToLineStart, Some("Editor")),
        KeyBinding::new("end", MoveToLineEnd, Some("Editor")),
        KeyBinding::new("cmd-left", MoveToLineStart, Some("Editor")),
        KeyBinding::new("cmd-right", MoveToLineEnd, Some("Editor")),
        KeyBinding::new("ctrl-a", MoveToLineStart, Some("Editor")),
        KeyBinding::new("ctrl-e", MoveToLineEnd, Some("Editor")),
        KeyBinding::new("shift-left", SelectLeft, Some("Editor")),
        KeyBinding::new("shift-right", SelectRight, Some("Editor")),
        KeyBinding::new("shift-up", SelectUp, Some("Editor")),
        KeyBinding::new("shift-down", SelectDown, Some("Editor")),
        KeyBinding::new("cmd-a", SelectAll, Some("Editor")),
        KeyBinding::new("cmd-z", Undo, Some("Editor")),
        KeyBinding::new("cmd-shift-z", Redo, Some("Editor")),
        KeyBinding::new("cmd-s", Save, Some("Editor")),
    ]);
}

/// 編集ビュー。M2 では 1 バッファを直接保持する（共有は後で `Entity<Buffer>` 化）。
pub struct EditorView {
    buffer: Buffer,
    focus_handle: FocusHandle,
    theme: Theme,
    /// キャレット・現在行アクセントの色（アクティブプロジェクト色。既定はパレット先頭）。
    accent: gpui::Hsla,
    /// 平坦モード（composer 用）: ガター・行番号・現在行ハイライト無し・UI フォント。
    plain: bool,
    /// Enter で送信するか（composer 専用）。true = Enter 送信 / Shift+Enter 改行、
    /// false = Enter 改行（送信は ⌘Enter）。**IME 変換中（marked_range あり）は送信しない**。
    submit_on_enter: bool,
    scroll_top: Pixels,
    /// IME 変換中テキストの byte レンジ（下線表示）。
    marked_range: Option<Range<usize>>,
    // 直近 paint のキャッシュ（ウィンドウ座標）。ヒットテスト・IME 位置・スクロールに使う。
    content_origin: Option<Point<Pixels>>,
    viewport_height: Pixels,
    gutter_width: Pixels,
    caret_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// 選択ドラッグ中の最新ポインタ位置（ビュー外自動スクロールの tick が読む）。
    drag_position: Option<Point<Pixels>>,
    /// ビュー外ドラッグの自動スクロール tick が走っているか（二重 spawn 防止）。
    drag_autoscroll_running: bool,
    /// ドラッグ選択の粒度（mouse down で決まり mouse move が読む）。
    drag_select: DragSelectMode,
    // キャレット点滅。focus 中のみ動かし、blur で止める（idle CPU 0% を守る）。
    blink_visible: bool,
    /// 親ビューが点滅を許可しているか。ACP の生成中など、入力先ではあっても視覚更新を
    /// 増やしたくない状態では false にする。非アクティブ窓は paint 側でも必ず停止する。
    caret_blink_enabled: bool,
    _blink_task: Option<gpui::Task<()>>,
    // 構文ハイライト（対応拡張子のみ・増分パース M11-8）。編集は Tree::edit + 差分パース、
    // span は prepaint が可視範囲だけ問い合わせる（512KB 上限は撤廃）。
    highlighter: Option<lang::IncrementalHighlighter>,
    /// ハイライト済みの buffer version（prepaint での増分/全文パースの判定）。
    highlight_version: u64,
    /// 検索ジャンプ等の保留スクロール（byte offset）。次の prepaint で viewport 確定後に消化＝one-shot。
    pending_reveal: Option<usize>,
    /// 編集/カーソル移動で要求された caret 可視化を、次の prepaint（wrap_map 再構築後）で消化する。
    /// 編集ハンドラ時点では wrap_map が旧 version で `display_row_for_offset` が論理行へフォールバックし、
    /// 折り返した長い行のキャレット表示行を誤る（＝画面外のまま）ため、reveal を prepaint へ遅延する。
    pending_caret_reveal: bool,
    /// gutter diff（HEAD vs 現在バッファ・非同期計算）。plain / 無題ファイルは常に空。
    diff_hunks: Vec<project::DiffHunk>,
    /// diff を最後にスケジュールしたバッファ version（prepaint での重複起動防止）。
    diff_scheduled_version: u64,
    /// diff デバウンスの世代（古い計算を無効化＝連続編集を 1 回に畳む）。
    diff_gen: u32,
    /// LSP 診断（行番号 + 重大度）。gutter の下線に使う。workspace が push する。
    diagnostics: Vec<(u32, lang::lsp::Severity)>,
    /// ⌘F の全マッチ（byte レンジ・start 昇順・非重複）。warn の面で選択面より下に塗る。
    /// workspace の検索バーが差し込む（このビューは検索自体を知らない）。
    search_ranges: Vec<Range<usize>>,
    /// blame 注釈（行, テキスト）。キャレット行の行末に dim 表示（M11-11）。workspace が差し込む。
    line_annotation: Option<(usize, SharedString)>,
    /// エージェント編集のスレッド色（M12-3 生中継）。Some の間、gutter diff バーをこの色で塗る。
    agent_mark_color: Option<gpui::Hsla>,
    /// 外部変更が来たが dirty のため自動リロードしなかった（警告バー表示中）。
    external_changed: bool,
    /// soft wrap（折り返し表示・⌥Z / 設定 `soft_wrap`）。plain（composer）では未使用。
    soft_wrap: bool,
    /// `.md` の整形プレビュー（source ⇄ rendered トグル・⌘⇧V）。plain / 非 md では常に false。
    /// true の間 EditorElement を描かず [`markdown_preview`] へ差し替える（キャレット点滅も止める）。
    rendered_markdown: bool,
    /// プレビューの縦スクロール位置（EditorElement の縦スクロールとは独立系統）。
    markdown_scroll: gpui::ScrollHandle,
    /// パース済みブロックのキャッシュ。version 変化時のみ再パース（idle 再描画で再解析しない）。
    markdown_blocks: Vec<markdown::Block>,
    markdown_blocks_version: u64,
    /// コードのフォントサイズ（settings の `font_size`・live 反映）。行高は 23/13 比で追従。
    font_size: f32,
    /// Tab 幅（settings の `tab_size`・live 反映）。
    tab_size: usize,
    /// 論理行↔表示行マップ（prepaint が (version, columns, on/off) 変化時に再構築）。
    wrap_map: WrapMap,
    /// hover dwell の世代（マウスが動くたび増える＝進行中の dwell タイマーを無効化）。
    hover_generation: u32,
    /// 直近 Dwell を emit した位置（ここから離れたら Cancel を emit）。
    last_dwell_anchor: Option<Point<Pixels>>,
    /// 進行中の dwell タイマー（差し替えで旧タスクは drop ＝キャンセル）。
    _hover_task: Option<gpui::Task<()>>,
}

/// ⌘F の Esc 復帰用に保存する位置（選択 + スクロール）。中身は編集で無効になりうるため
/// 復元時にバッファ長へクリップされる。[`EditorView::position_snapshot`] で採取する。
pub struct PositionSnapshot {
    selections: Vec<Selection>,
    scroll_top: Pixels,
}

impl EditorView {
    pub fn new(buffer: Buffer, theme: Theme, accent: gpui::Hsla, cx: &mut Context<Self>) -> Self {
        let mut highlighter = buffer
            .path()
            .and_then(lang::IncrementalHighlighter::for_path);
        if let Some(highlighter) = highlighter.as_mut() {
            highlighter.reparse_full(&buffer.text()); // 開いた直後の全文パース（以後は増分）
        }
        let highlight_version = buffer.version();
        Self {
            buffer,
            focus_handle: cx.focus_handle(),
            theme,
            accent,
            plain: false,
            submit_on_enter: false,
            highlighter,
            highlight_version,
            pending_reveal: None,
            pending_caret_reveal: false,
            diff_hunks: Vec::new(),
            diff_scheduled_version: u64::MAX, // 初回描画で必ず計算させる
            diff_gen: 0,
            diagnostics: Vec::new(),
            search_ranges: Vec::new(),
            line_annotation: None,
            agent_mark_color: None,
            external_changed: false,
            soft_wrap: false,
            rendered_markdown: false,
            markdown_scroll: gpui::ScrollHandle::new(),
            markdown_blocks: Vec::new(),
            markdown_blocks_version: u64::MAX,
            font_size: FONT_SIZE,
            tab_size: DEFAULT_TAB_SIZE,
            wrap_map: WrapMap::identity(1, (u64::MAX, 0, false)),
            hover_generation: 0,
            last_dwell_anchor: None,
            _hover_task: None,
            scroll_top: px(0.),
            marked_range: None,
            content_origin: None,
            viewport_height: px(0.),
            gutter_width: px(0.),
            caret_bounds: None,
            is_selecting: false,
            drag_position: None,
            drag_autoscroll_running: false,
            drag_select: DragSelectMode::Char,
            blink_visible: true,
            caret_blink_enabled: true,
            _blink_task: None,
        }
    }

    /// キャレット点滅を開始する（focus 時に呼ぶ）。530ms ごとに反転して notify。
    /// `_blink_task` を差し替える＝古いタスクは drop されて止まる。blur 時は `_blink_task=None`。
    fn start_blinking(&mut self, cx: &mut Context<Self>) {
        self.blink_visible = true;
        self._blink_task = Some(cx.spawn(async move |editor, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(530))
                .await;
            let alive = editor
                .update(cx, |editor, cx| {
                    editor.blink_visible = !editor.blink_visible;
                    cx.notify();
                })
                .is_ok();
            if !alive {
                break;
            }
        }));
    }

    /// composer 用の平坦な空エディタ（ガター・行番号・現在行なし・UI フォント）。IME/編集ロジックは共通。
    /// `submit_on_enter` は Enter 送信の初期値（設定 [`settings_core::Settings::submit_on_enter`] を流す）。
    pub fn plain(
        theme: Theme,
        accent: gpui::Hsla,
        submit_on_enter: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(Buffer::new(), theme, accent, cx);
        view.plain = true;
        view.submit_on_enter = submit_on_enter;
        view
    }

    /// Enter 送信の有効/無効を切り替える（設定変更・アプリ内トグルから）。composer 専用。
    pub fn set_submit_on_enter(&mut self, value: bool, cx: &mut Context<Self>) {
        self.submit_on_enter = value;
        cx.notify();
    }

    /// 現在 Enter 送信が有効か（親が送信ボタンのヒント表示に使う）。
    pub fn submit_on_enter(&self) -> bool {
        self.submit_on_enter
    }

    /// キャレットの無限点滅だけを有効／無効にする。無効中もキャレット自体は常時表示する。
    /// ACP composer は生成中に false を渡し、入力・選択などの Editor 機能はそのまま保つ。
    pub fn set_caret_blink_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.caret_blink_enabled == enabled {
            return;
        }
        self.caret_blink_enabled = enabled;
        self.blink_visible = true;
        if !enabled {
            self._blink_task = None;
        }
        cx.notify();
    }

    pub fn caret_blink_enabled(&self) -> bool {
        self.caret_blink_enabled
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// 指定 (行, 列)（0 始まり）へキャレットを移動し、対象行を可視域の中央へ寄せる（検索ジャンプ用）。
    /// viewport 未確定（初回描画前）でも動くよう、実スクロールは次の prepaint が [`Self::pending_reveal`]
    /// を消化して決める（one-shot＝settle 後は再描画しない＝idle 0%）。
    pub fn reveal_position(&mut self, row: usize, column: usize, cx: &mut Context<Self>) {
        let snapshot = self.buffer.snapshot();
        let clamped_row = row.min(snapshot.line_count().saturating_sub(1));
        let offset =
            snapshot.clip_offset(snapshot.point_to_byte(BufferPoint::new(clamped_row, column)));
        self.buffer.set_selections(vec![Selection::cursor(offset)]);
        self.blink_visible = true;
        self.pending_reveal = Some(offset);
        cx.notify();
    }

    /// エージェント編集のスレッド色（gutter の diff バー色を差し替える・M12-3）。None で通常色へ。
    pub fn set_agent_mark_color(&mut self, color: Option<gpui::Hsla>, cx: &mut Context<Self>) {
        if self.agent_mark_color != color {
            self.agent_mark_color = color;
            cx.notify();
        }
    }

    /// blame 注釈を差し替える（workspace が計算して呼ぶ・M11-11）。None でクリア。
    pub fn set_line_annotation(
        &mut self,
        annotation: Option<(usize, SharedString)>,
        cx: &mut Context<Self>,
    ) {
        if self.line_annotation != annotation {
            self.line_annotation = annotation;
            cx.notify();
        }
    }

    /// バッファを読み取り専用にする（diff タブ・M11-9）。
    pub fn set_read_only(&mut self, read_only: bool) {
        self.buffer.set_read_only(read_only);
    }

    /// ⌘F の全マッチハイライトを差し替える（workspace の検索バーが呼ぶ）。空 Vec でクリア。
    /// 同じ内容なら何もしない（observe 再入で無駄な再描画をしないため）。
    pub fn set_search_ranges(&mut self, ranges: Vec<Range<usize>>, cx: &mut Context<Self>) {
        if self.search_ranges == ranges {
            return;
        }
        self.search_ranges = ranges;
        cx.notify();
    }

    /// byte レンジを選択して見せる（⌘F の現在マッチ）。行が可視域の外なら中央へスクロール。
    pub fn select_byte_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let snapshot = self.buffer.snapshot();
        let start = snapshot.clip_offset(range.start);
        let end = snapshot.clip_offset(range.end);
        self.buffer.set_selections(vec![Selection::new(start, end)]);
        self.blink_visible = true;
        self.pending_reveal = Some(start);
        cx.notify();
    }

    /// 現在の位置（選択 + スクロール）を保存する（⌘F を開く時に採取 → Esc で戻す）。
    pub fn position_snapshot(&self) -> PositionSnapshot {
        PositionSnapshot {
            selections: self.buffer.selections().to_vec(),
            scroll_top: self.scroll_top,
        }
    }

    /// [`Self::position_snapshot`] の位置へ戻す（⌘F の Esc）。編集で無効になった分はクリップ。
    pub fn restore_position(&mut self, snapshot: &PositionSnapshot, cx: &mut Context<Self>) {
        self.buffer.set_selections(snapshot.selections.clone());
        self.scroll_top = snapshot.scroll_top.max(px(0.)).min(self.max_scroll_top());
        self.pending_reveal = None;
        self.blink_visible = true;
        cx.notify();
    }

    /// レンジ群を同一テキストで置換する（⌘F の置換。複数レンジでも 1 Transaction ＝ undo 一発）。
    pub fn replace_ranges(&mut self, ranges: &[Range<usize>], text: &str, cx: &mut Context<Self>) {
        if ranges.is_empty() {
            return;
        }
        self.buffer.edit(ranges, text);
        self.after_edit(cx);
    }

    /// settings の font_size / tab_size を適用する（live 反映・M10-13）。
    pub fn set_typography(&mut self, font_size: f32, tab_size: usize, cx: &mut Context<Self>) {
        let font_size = font_size.clamp(8.0, 32.0);
        let tab_size = tab_size.clamp(1, 16);
        if (self.font_size - font_size).abs() > f32::EPSILON || self.tab_size != tab_size {
            self.font_size = font_size;
            self.tab_size = tab_size;
            // 行高が変わる＝折返し列数は同じでも wrap マップの再構築は不要（列はセル数基準）。
            cx.notify();
        }
    }

    /// soft wrap の on/off（設定の適用）。次の prepaint でマップが作り直される。
    pub fn set_soft_wrap(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.soft_wrap != enabled {
            self.soft_wrap = enabled;
            cx.notify();
        }
    }

    /// ⌥Z トグル。
    fn toggle_soft_wrap(&mut self, _: &ToggleSoftWrap, _: &mut Window, cx: &mut Context<Self>) {
        self.soft_wrap = !self.soft_wrap;
        cx.notify();
    }

    /// このバッファが markdown か（プレビュー可否）。無題・非対応拡張子は false。
    fn is_markdown(&self) -> bool {
        self.buffer
            .path()
            .map(|path| lang::language_for_path(path) == Some(lang::LanguageId::Markdown))
            .unwrap_or(false)
    }

    /// ⌘⇧V: source ⇄ rendered 整形プレビューのトグル（markdown ファイルのみ）。
    fn toggle_rendered_markdown(
        &mut self,
        _: &ToggleRenderedMarkdown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_markdown() {
            self.set_rendered_markdown(!self.rendered_markdown, cx);
        }
    }

    /// 整形プレビュー中か（クローム側のトグルボタンが状態表示に読む）。
    pub fn rendered_markdown(&self) -> bool {
        self.rendered_markdown
    }

    /// 整形プレビューの on/off（トグルハンドラ / offscreen probe から）。非 md での ON は無視。
    /// ON では EditorElement を描かない＝点滅の paint 側管理（should_blink）が走らないので、
    /// ここで点滅タスクを明示停止して idle 再描画を止める（点滅停止＝idle CPU 予算を守る）。
    pub fn set_rendered_markdown(&mut self, on: bool, cx: &mut Context<Self>) {
        if on && !self.is_markdown() {
            return;
        }
        if self.rendered_markdown == on {
            return;
        }
        self.rendered_markdown = on;
        if on {
            self._blink_task = None;
            self.blink_visible = true;
        }
        cx.notify();
    }

    /// offset の**表示行**（wrap 有効時）。マップが古い（直後の prepaint 前）は論理行へフォールバック。
    fn display_row_for_offset(&self, snapshot: &BufferSnapshot, offset: usize) -> usize {
        let point = snapshot.byte_to_point(offset);
        if self.wrap_map.key.0 == snapshot.version() {
            self.wrap_map.logical_to_display(point.row, point.column)
        } else {
            point.row
        }
    }

    /// 総表示行数（マップが古ければ論理行数）。
    fn total_display_rows(&self, snapshot: &BufferSnapshot) -> usize {
        if self.wrap_map.key.0 == snapshot.version() {
            self.wrap_map.total_display_rows()
        } else {
            snapshot.line_count()
        }
    }

    /// 表示行単位の縦移動先 byte（wrap 中の行内移動）。マップが古ければ論理行移動。
    fn vertical_display_target(
        &self,
        snapshot: &BufferSnapshot,
        head: usize,
        delta: isize,
    ) -> usize {
        if self.wrap_map.key.0 != snapshot.version() {
            return vertical(snapshot, head, delta);
        }
        let point = snapshot.byte_to_point(head);
        let display = self.wrap_map.logical_to_display(point.row, point.column);
        let total = self.wrap_map.total_display_rows();
        let target = (display as isize + delta).clamp(0, total.saturating_sub(1) as isize) as usize;
        if target == display {
            return head;
        }
        let (line, segment) = self.wrap_map.display_to_logical(target);
        let line_len = snapshot.line_len_bytes(line);
        let target_range = self.wrap_map.segment_range(line, segment, line_len);
        // 現セグメント内の相対 byte を維持（等幅近似・行末クリップ）。
        let current_range = {
            let (c_line, c_segment) = self.wrap_map.display_to_logical(display);
            self.wrap_map
                .segment_range(c_line, c_segment, snapshot.line_len_bytes(c_line))
        };
        let relative = point.column.saturating_sub(current_range.start);
        let column = (target_range.start + relative).min(target_range.end);
        snapshot.clip_offset(snapshot.point_to_byte(BufferPoint::new(line, column)))
    }

    /// テーマを差し替える（テーマセレクタのライブプレビュー / 切替）。次の描画で新配色になる。
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// LSP 診断（行 + 重大度）を差し替える（workspace が publishDiagnostics 受信時に push）。
    pub fn set_diagnostics(
        &mut self,
        diagnostics: Vec<(u32, lang::lsp::Severity)>,
        cx: &mut Context<Self>,
    ) {
        self.diagnostics = diagnostics;
        cx.notify();
    }

    /// 主キャレットの LSP 位置 `(line, character)`（character は行頭からの **UTF-16 code unit** 数）。
    /// 補完/hover/定義の要求に渡す。
    pub fn cursor_lsp_position(&self) -> (u32, u32) {
        self.lsp_position_for_offset(self.primary().head)
    }

    /// 任意の byte offset の LSP 位置 `(line, character〔UTF-16〕)`（hover のマウス位置用）。
    pub fn lsp_position_for_offset(&self, offset: usize) -> (u32, u32) {
        let snapshot = self.buffer.snapshot();
        let offset = snapshot.clip_offset(offset);
        let point = snapshot.byte_to_point(offset);
        let line_start = snapshot.point_to_byte(BufferPoint::new(point.row, 0));
        let utf16_line_start = self.buffer.byte_to_utf16(line_start);
        let utf16_offset = self.buffer.byte_to_utf16(offset);
        (point.row as u32, (utf16_offset - utf16_line_start) as u32)
    }

    /// LSP 位置 `(line, character〔UTF-16〕)` へジャンプする（定義ジャンプの着地）。
    /// UTF-16 → byte 列に直して [`Self::reveal_position`] に委譲する。
    pub fn reveal_lsp_position(&mut self, line: u32, character: u32, cx: &mut Context<Self>) {
        let snapshot = self.buffer.snapshot();
        let row = (line as usize).min(snapshot.line_count().saturating_sub(1));
        let line_start = snapshot.point_to_byte(BufferPoint::new(row, 0));
        let utf16_line_start = self.buffer.byte_to_utf16(line_start);
        let target = self
            .buffer
            .utf16_to_byte(utf16_line_start + character as usize);
        let column = target.saturating_sub(line_start);
        self.reveal_position(row, column, cx);
    }

    /// 主キャレットのウィンドウ座標（下端左）。補完ポップアップの表示位置に使う。直近 paint 由来。
    pub fn caret_window_position(&self) -> Option<Point<Pixels>> {
        self.caret_bounds
            .map(|bounds| point(bounds.left(), bounds.bottom()))
    }

    /// カーソル直前の識別子プレフィクス（ASCII 英数 or `_` の連なり）。
    /// 戻り値は `(語頭の byte offset, プレフィクス文字列)`。語の途中でなければ `(caret, "")`。
    /// 補完の適用・自動トリガの絞り込み・Esc 抑止の語判定に共通で使う。
    pub fn identifier_prefix_at_caret(&self) -> (usize, String) {
        let head = self.primary().head;
        let all = self.buffer.text();
        let bytes = all.as_bytes();
        let mut start = head;
        while start > 0 {
            let previous = bytes[start - 1];
            if previous.is_ascii_alphanumeric() || previous == b'_' {
                start -= 1;
            } else {
                break;
            }
        }
        (start, all[start..head].to_string())
    }

    /// カーソル直前の最大 `max_bytes` バイトのテキスト（`::` などのトリガ文字判定用）。
    pub fn text_before_caret(&self, max_bytes: usize) -> String {
        let head = self.primary().head;
        self.buffer.text_range(head.saturating_sub(max_bytes)..head)
    }

    /// バッファ全文を置き換える（hot exit の復元）。1 Transaction ＝ undo で復元前に戻れる。
    pub fn replace_all_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let len = self.buffer.len_bytes();
        self.buffer.edit(&[0..len], text);
        self.after_edit(cx);
    }

    /// Backspace 1 回分（補完ポップアップの type-through 用。アクション経由と同じ後処理）。
    pub fn delete_backward_char(&mut self, cx: &mut Context<Self>) {
        self.buffer.delete_backward();
        self.after_edit(cx);
    }

    /// テキストをキャレット位置へ挿入する（補完ポップアップの type-through 用）。
    /// 通常タイプと同じ後処理 + [`EditorInputEvent::Typed`] を emit する（自動トリガが再絞り込みに使う）。
    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.buffer.insert(text);
        self.after_edit(cx);
        cx.emit(EditorInputEvent::Typed(text.to_string()));
    }

    /// LSP 位置レンジ（UTF-16 line/char）を byte レンジへ（フォーマット/rename の TextEdit 適用用）。
    pub fn lsp_range_to_bytes(
        &self,
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
    ) -> Range<usize> {
        let snapshot = self.buffer.snapshot();
        let to_byte = |line: u32, character: u32| -> usize {
            let row = (line as usize).min(snapshot.line_count().saturating_sub(1));
            let line_start = snapshot.point_to_byte(BufferPoint::new(row, 0));
            let utf16_line_start = self.buffer.byte_to_utf16(line_start);
            self.buffer
                .utf16_to_byte(utf16_line_start + character as usize)
        };
        let start = to_byte(start_line, start_character);
        let end = to_byte(end_line, end_character).max(start);
        start..end
    }

    /// LSP TextEdit 群（byte レンジ変換済み）を 1 Transaction で適用し、キャレット位置を概ね保つ。
    pub fn apply_lsp_edits(&mut self, edits: Vec<(Range<usize>, String)>, cx: &mut Context<Self>) {
        if edits.is_empty() {
            return;
        }
        // 編集前のキャレット (行, 列) を保存 → 適用後に同じ座標へクランプして戻す。
        let snapshot = self.buffer.snapshot();
        let caret_point = snapshot.byte_to_point(self.primary().head);
        let scroll_top = self.scroll_top;
        self.buffer.edit_batch(&edits);
        let snapshot = self.buffer.snapshot();
        let restored = snapshot.point_to_byte(BufferPoint::new(
            caret_point.row.min(snapshot.line_count().saturating_sub(1)),
            caret_point.column,
        ));
        self.buffer
            .set_selections(vec![Selection::cursor(restored)]);
        self.after_edit(cx);
        // フォーマットで画面が飛ばないようスクロールは維持（クランプのみ）。
        self.scroll_top = scroll_top.max(px(0.)).min(self.max_scroll_top());
    }

    /// 補完を適用する（カーソル直前の識別子プレフィクスを `text` で置換）。
    pub fn apply_completion(&mut self, text: &str, cx: &mut Context<Self>) {
        let head = self.primary().head;
        let (start, _) = self.identifier_prefix_at_caret();
        self.buffer.edit(&[start..head], text);
        let new_cursor = start + text.len();
        self.buffer
            .set_selections(vec![Selection::cursor(new_cursor)]);
        self.after_edit(cx);
    }

    // ── 外部変更の追従（watch 基盤・M10） ──

    /// watch イベント（このファイルに何かが起きた）を処理する。
    /// 自分の保存によるイベント（revision 一致）は無視。無編集なら自動リロード、
    /// dirty なら警告バー用のフラグを立てる（上書きは絶対にしない）。
    pub fn handle_external_change(&mut self, cx: &mut Context<Self>) {
        match self.buffer.disk_probably_unchanged() {
            Some(true) => {
                // 自分の保存 or 既に同期済み。
                if self.external_changed {
                    self.external_changed = false;
                    cx.notify();
                }
            }
            Some(false) if !self.buffer.is_dirty() => self.reload_from_disk(cx),
            Some(false) => {
                self.external_changed = true;
                cx.notify();
            }
            // 削除・アクセス不能: dirty なら警告（保存し直せる）、無編集なら何もしない（v1）。
            None => {
                if self.buffer.is_dirty() && self.buffer.path().is_some() {
                    self.external_changed = true;
                    cx.notify();
                }
            }
        }
    }

    /// ディスクから読み直す（自動リロード / 警告バーの「再読込」）。スクロールは維持しつつクランプ。
    pub fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.buffer.reload() {
            eprintln!("再読込に失敗: {error:#}");
            return;
        }
        self.external_changed = false;
        self.marked_range = None;
        self.refresh_highlights();
        self.scroll_top = self.scroll_top.max(px(0.)).min(self.max_scroll_top());
        cx.notify();
    }

    /// 警告バーの「このまま」= 警告だけ畳む（保存時の競合検知が最後の砦として残る）。
    pub fn dismiss_external_change(&mut self, cx: &mut Context<Self>) {
        if self.external_changed {
            self.external_changed = false;
            cx.notify();
        }
    }

    /// 外部変更の警告バーを出すべきか。
    pub fn is_externally_changed(&self) -> bool {
        self.external_changed
    }

    /// gutter diff を明示的に再計算する（watch が git 状態の変化を検知したとき等）。
    pub fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        self.schedule_diff(cx);
    }

    /// gutter diff（HEAD vs 現在バッファ）を再計算する。編集で version が変わるたび prepaint から呼ぶ。
    /// デバウンス 250ms・git 実行は背景スレッド・連続編集は世代番号で 1 回に畳む（idle 0% を守る）。
    /// plain / 無題ファイルは diff を持たない。
    fn schedule_diff(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.buffer.path().map(|path| path.to_path_buf()) else {
            if !self.diff_hunks.is_empty() {
                self.diff_hunks.clear();
                cx.notify();
            }
            return;
        };
        self.diff_gen = self.diff_gen.wrapping_add(1);
        let generation = self.diff_gen;
        let text = self.buffer.text();
        let host = self.buffer.host().clone();
        cx.spawn(async move |editor, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            // まだ最新の編集か（後続の編集が来ていたら破棄＝デバウンス）。
            let latest = editor
                .update(cx, |editor, _| editor.diff_gen == generation)
                .unwrap_or(false);
            if !latest {
                return;
            }
            // git 呼び出し（ブロッキング）は背景スレッドで実行。
            let hunks = cx
                .background_executor()
                .spawn(async move { project::buffer_diff_on(host.as_ref(), &path, &text) })
                .await;
            let _ = editor.update(cx, |editor, cx| {
                if editor.diff_gen == generation {
                    editor.diff_hunks = hunks;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 現在のテキスト全体（composer が送信時に読む）。
    pub fn plain_text(&self) -> String {
        self.buffer.text()
    }

    /// テキストを差し替える（composer の下書き流し込み・開発プローブ用）。
    pub fn set_plain_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.buffer = Buffer::from_str(text);
        self.buffer
            .set_selections(vec![Selection::cursor(text.len())]);
        self.marked_range = None;
        self.highlight_version = self.buffer.version();
        self.pending_caret_reveal = true; // 末尾キャレットを可視域へ（折り返し時も）
        cx.notify();
    }

    /// テキストを空に戻す（composer 送信後）。
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.buffer = Buffer::new();
        self.marked_range = None;
        self.highlight_version = self.buffer.version();
        if let Some(highlighter) = self.highlighter.as_mut() {
            highlighter.reparse_full("");
        }
        self.scroll_top = px(0.);
        cx.notify();
    }

    /// statusbar 用の 1 始まりカーソル位置 `(行, 列)`。列は行内の**文字数**（byte ではない）。
    pub fn cursor_display(&self) -> (usize, usize) {
        let head = self.primary().head;
        let snapshot = self.buffer.snapshot();
        let point = snapshot.byte_to_point(head);
        let line_text = snapshot.line_text(point.row);
        let column = line_text
            .get(..point.column.min(line_text.len()))
            .map(|prefix| prefix.chars().count())
            .unwrap_or(0);
        (point.row + 1, column + 1)
    }

    /// statusbar 用の言語ラベル（拡張子から。未知の拡張子は大文字化、拡張子無しは `None`）。
    pub fn language_label(&self) -> Option<SharedString> {
        let path = self.buffer.path()?;
        if let Some(language) = lang::language_for_path(path) {
            return Some(SharedString::from(language.label()));
        }
        let extension = path.extension()?.to_str()?;
        let name = if extension.eq_ignore_ascii_case("txt") {
            "Text".to_string()
        } else {
            extension.to_uppercase()
        };
        Some(SharedString::from(name))
    }

    /// キャレット・現在行アクセント色を設定する（アクティブプロジェクト色を流す）。
    pub fn set_accent(&mut self, color: gpui::Hsla, cx: &mut Context<Self>) {
        self.accent = color;
        cx.notify();
    }

    fn line_height(&self) -> Pixels {
        px(self.line_height_value())
    }

    /// 現在の内容の表示高さ（折り返し後の表示行数 × 行高）。composer の auto-grow が読む。
    /// wrap マップが古い間（直後の prepaint 前）は論理行数ベースの近似になるが、
    /// 次フレームで正値に収束する。
    pub fn content_height(&self) -> Pixels {
        let snapshot = self.buffer.snapshot();
        self.line_height() * self.total_display_rows(&snapshot) as f32
    }

    /// 行高の実値（font_size × 23/13 比）。prepaint・ヒットテスト・スクロール計算はこれを使う。
    fn line_height_value(&self) -> f32 {
        self.font_size * (LINE_HEIGHT / FONT_SIZE)
    }

    fn primary(&self) -> Selection {
        self.buffer
            .selections()
            .first()
            .copied()
            .unwrap_or(Selection::cursor(0))
    }

    // ── カーソル移動 ──

    fn apply_cursor<F>(&mut self, extend: bool, compute: F, cx: &mut Context<Self>)
    where
        F: Fn(&BufferSnapshot, &Selection) -> usize,
    {
        let snapshot = self.buffer.snapshot();
        let moved: Vec<Selection> = self
            .buffer
            .selections()
            .iter()
            .map(|selection| {
                let head = compute(&snapshot, selection);
                if extend {
                    Selection::new(selection.anchor, head)
                } else {
                    Selection::cursor(head)
                }
            })
            .collect();
        self.buffer.set_selections(moved);
        self.marked_range = None;
        self.blink_visible = true; // カーソル移動直後はキャレットを実体化
        self.pending_caret_reveal = true;
        cx.notify();
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            false,
            |snapshot, selection| {
                if selection.is_empty() {
                    snapshot.prev_char_boundary(selection.head)
                } else {
                    selection.start()
                }
            },
            cx,
        );
    }

    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            false,
            |snapshot, selection| {
                if selection.is_empty() {
                    snapshot.next_char_boundary(selection.head)
                } else {
                    selection.end()
                }
            },
            cx,
        );
    }

    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, false, cx);
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, false, cx);
    }

    /// 表示行単位の縦移動（soft wrap 対応・M10-12）。off なら論理行と一致する。
    fn move_vertical(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let snapshot = self.buffer.snapshot();
        let len = self.buffer.len_bytes();
        let moved: Vec<Selection> = self
            .buffer
            .selections()
            .iter()
            .map(|selection| {
                let mut head = self.vertical_display_target(&snapshot, selection.head, delta);
                // 平坦入力（composer 等）では、先頭行での↑・末尾行での↓を文書の頭/末へ寄せる
                // ＝チャット入力の定番。縦移動できなかった（＝端に達した）ときだけ効かせるので、
                // 途中行では通常の行移動のまま。
                if self.plain && head == selection.head {
                    head = if delta < 0 { 0 } else { len };
                }
                if extend {
                    Selection::new(selection.anchor, head)
                } else {
                    Selection::cursor(head)
                }
            })
            .collect();
        self.buffer.set_selections(moved);
        self.marked_range = None;
        self.blink_visible = true;
        self.pending_caret_reveal = true;
        cx.notify();
    }

    fn move_to_line_start(&mut self, _: &MoveToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            false,
            |snapshot, selection| line_edge(snapshot, selection.head, false),
            cx,
        );
    }

    fn move_to_line_end(&mut self, _: &MoveToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            false,
            |snapshot, selection| line_edge(snapshot, selection.head, true),
            cx,
        );
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            true,
            |snapshot, selection| snapshot.prev_char_boundary(selection.head),
            cx,
        );
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            true,
            |snapshot, selection| snapshot.next_char_boundary(selection.head),
            cx,
        );
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, true, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, true, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.buffer.len_bytes();
        self.buffer.set_selections(vec![Selection::new(0, len)]);
        self.marked_range = None;
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let selection = self.primary();
        if selection.is_empty() {
            // 空選択のコピーは消費せず親へ譲る。GPUI の action は leaf→root で流れるので、
            // 譲ることで agent_panel が transcript のドラッグ選択コピーを拾える（M13）。
            cx.propagate();
            return;
        }
        let text = self.buffer.text_range(selection.range());
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let selection = self.primary();
        if selection.is_empty() {
            return;
        }
        let text = self.buffer.text_range(selection.range());
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.buffer.insert(""); // 選択を空で置換＝削除
        self.after_edit(cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.buffer.insert(&text);
            self.after_edit(cx);
        }
    }

    // ── 編集 ──

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_backward();
        self.after_edit(cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_forward();
        self.after_edit(cx);
    }

    // ── 編集の所作（M10-9）: 単語移動/削除・行操作・コメント・インデント ──

    fn move_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            false,
            |snapshot, selection| snapshot.prev_word_boundary(selection.head),
            cx,
        );
    }

    fn move_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            false,
            |snapshot, selection| snapshot.next_word_boundary(selection.head),
            cx,
        );
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            true,
            |snapshot, selection| snapshot.prev_word_boundary(selection.head),
            cx,
        );
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(
            true,
            |snapshot, selection| snapshot.next_word_boundary(selection.head),
            cx,
        );
    }

    fn delete_word_backward(
        &mut self,
        _: &DeleteWordBackward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffer.delete_word_backward();
        self.after_edit(cx);
    }

    fn move_to_start(&mut self, _: &MoveToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(false, |_, _| 0, cx);
    }

    fn move_to_end(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(false, |snapshot, _| snapshot.len_bytes(), cx);
    }

    fn move_line_up(&mut self, _: &MoveLineUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.move_lines(false).is_some() {
            self.after_edit(cx);
        }
    }

    fn move_line_down(&mut self, _: &MoveLineDown, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.move_lines(true).is_some() {
            self.after_edit(cx);
        }
    }

    fn duplicate_line_up(&mut self, _: &DuplicateLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.duplicate_lines(false);
        self.after_edit(cx);
    }

    fn duplicate_line_down(
        &mut self,
        _: &DuplicateLineDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffer.duplicate_lines(true);
        self.after_edit(cx);
    }

    fn delete_line(&mut self, _: &DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_lines();
        self.after_edit(cx);
    }

    /// ⌘/ コメントトグル（言語別 prefix・対応言語のみ）。
    fn toggle_comment(&mut self, _: &ToggleComment, _: &mut Window, cx: &mut Context<Self>) {
        let Some(prefix) = self
            .buffer
            .path()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .and_then(lang::comment_prefix)
        else {
            return;
        };
        self.buffer.toggle_comment(prefix);
        self.after_edit(cx);
    }

    /// Tab: 選択があれば行インデント・なければ空白挿入（`DEFAULT_TAB_SIZE`。設定配線は M10-13）。
    fn tab_indent(&mut self, _: &TabIndent, _: &mut Window, cx: &mut Context<Self>) {
        let any_selection = self
            .buffer
            .selections()
            .iter()
            .any(|selection| !selection.is_empty());
        if any_selection {
            self.buffer.indent_lines(self.tab_size);
        } else {
            self.buffer.insert(&" ".repeat(self.tab_size));
        }
        self.after_edit(cx);
    }

    fn indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.indent_lines(self.tab_size);
        self.after_edit(cx);
    }

    fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.outdent_lines(self.tab_size);
        self.after_edit(cx);
    }

    // ── multi-cursor（M10-10） ──

    /// ⌘D。単語選択 → 次の一致を追加選択。新しい選択が見えるようにスクロール。
    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.select_next_occurrence() {
            if let Some(last) = self.buffer.selections().iter().map(|s| s.end()).max() {
                self.pending_reveal = Some(last);
            }
            self.blink_visible = true;
            cx.notify();
        }
    }

    fn add_cursor_above(&mut self, _: &AddCursorAbove, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.add_cursor_vertically(false) {
            self.blink_visible = true;
            cx.notify();
        }
    }

    fn add_cursor_below(&mut self, _: &AddCursorBelow, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.add_cursor_vertically(true) {
            self.blink_visible = true;
            cx.notify();
        }
    }

    /// Esc。複数選択を 1 個へ畳む（単一なら何もしない＝他のオーバーレイの Esc を邪魔しない）。
    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.collapse_to_primary() {
            self.blink_visible = true;
            cx.notify();
        }
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        // Enter 送信が有効なら、送信を親へ委ねる。ただし **IME 変換中（marked_range あり）は送信しない**
        // ＝日本語の変換確定 Enter で誤送信しないための肝（`docs/JOURNAL.md` の痛点）。
        if self.submit_on_enter && self.marked_range.is_none() {
            cx.emit(ComposerEvent::Submit);
            return;
        }
        if self.plain {
            self.buffer.insert("\n");
        } else {
            // 自動インデント（前行継承 + ブロック開始で 1 段。M10-9）。
            self.buffer.insert_newline_indented(self.tab_size);
        }
        self.after_edit(cx);
    }

    /// 常に改行を入れる（Enter 送信が有効でも改行したい時＝Shift+Enter）。送信は絶対にしない。
    fn insert_newline(&mut self, _: &InsertNewline, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.insert("\n");
        self.after_edit(cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.undo();
        self.after_edit(cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.redo();
        self.after_edit(cx);
    }

    /// ⌘S 保存。書き込みは背景スレッド（remote は 30s ブロックしうる — ARCHITECTURE §9）。
    /// 保存中に編集された場合は dirty のまま残る（`complete_save` の version 比較）。
    /// 保存（背景書き込み）。workspace の SaveActive / 保存時フォーマットから呼ぶ公開版。
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        self.save_impl(cx);
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.save_impl(cx);
    }

    fn save_impl(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.buffer.prepare_save() else {
            eprintln!("保存先が未設定（無題バッファ）");
            return;
        };
        let saved_version = pending.version;
        cx.spawn(async move |editor, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { pending.write() })
                .await;
            let _ = editor.update(cx, |editor, cx| {
                match result {
                    Ok(revision) => editor.buffer.complete_save(revision, saved_version),
                    Err(error) => eprintln!("保存に失敗: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.marked_range = None;
        self.blink_visible = true; // 編集直後はキャレットを実体化（点滅 OFF で消えないよう）
        self.refresh_highlights();
        self.pending_caret_reveal = true;
        cx.notify();
    }

    /// ハイライトの追従は prepaint が行う（増分パース・M11-8）。version 比較で必要時のみ。
    fn refresh_highlights(&mut self) {
        // 何もしない（互換のため残置。パースは prepaint の sync_highlight_tree で）。
    }

    /// prepaint 冒頭で呼ぶ: buffer version が進んでいたら Tree を追従させる。
    /// 単一編集は増分（Tree::edit + 差分パース）・それ以外（複数編集/undo/redo/reload）は全文。
    fn sync_highlight_tree(&mut self) {
        let version = self.buffer.version();
        if version == self.highlight_version {
            return;
        }
        self.highlight_version = version;
        let Some(highlighter) = self.highlighter.as_mut() else {
            return;
        };
        let text = self.buffer.text();
        match self.buffer.last_change() {
            Some(edits) if edits.len() == 1 => {
                let (start, old, new) = (&edits[0].0, &edits[0].1, &edits[0].2);
                highlighter.apply_edit(&text, *start, old, new);
            }
            _ => highlighter.reparse_full(&text),
        }
    }

    // ── スクロール ──

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta_y = event.delta.pixel_delta(self.line_height()).y;
        self.scroll_top = (self.scroll_top - delta_y)
            .max(px(0.))
            .min(self.max_scroll_top());
        self.cancel_hover(cx); // スクロールでアンカーがずれる → hover は消す
        cx.notify();
    }

    fn max_scroll_top(&self) -> Pixels {
        let snapshot = self.buffer.snapshot();
        let total = self.line_height() * self.total_display_rows(&snapshot) as f32;
        (total - self.viewport_height).max(px(0.))
    }

    fn scroll_caret_into_view(&mut self) {
        if self.viewport_height <= px(0.) {
            return;
        }
        let snapshot = self.buffer.snapshot();
        let row = self.display_row_for_offset(&snapshot, self.primary().head);
        let line_height = self.line_height();
        let caret_top = line_height * row as f32;
        let caret_bottom = caret_top + line_height;
        if caret_top < self.scroll_top {
            self.scroll_top = caret_top;
        } else if caret_bottom > self.scroll_top + self.viewport_height {
            self.scroll_top = caret_bottom - self.viewport_height;
        }
        self.scroll_top = self.scroll_top.max(px(0.)).min(self.max_scroll_top());
    }

    // ── マウス ──

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.blink_visible = true; // クリックでキャレットを置いた直後は実体化
        self.cancel_hover(cx);
        let offset = self.offset_for_position(event.position, window);
        // gutter の diff バー領域（左端 ~10px）をクリック → hunk 操作ポップオーバー（M11-10）。
        if !self.plain {
            if let Some(origin) = self.content_origin {
                let gutter_left = origin.x - self.gutter_width;
                if event.position.x >= gutter_left && event.position.x < gutter_left + px(10.) {
                    let snapshot = self.buffer.snapshot();
                    let relative_y = (event.position.y - origin.y + self.scroll_top).max(px(0.));
                    let display_row =
                        (f32::from(relative_y) / self.line_height_value()).floor() as usize;
                    let (row, _) = if self.wrap_map.key.0 == snapshot.version() {
                        self.wrap_map.display_to_logical(display_row)
                    } else {
                        (display_row.min(snapshot.line_count().saturating_sub(1)), 0)
                    };
                    let row32 = row as u32;
                    if let Some(hunk) = self
                        .diff_hunks
                        .iter()
                        .find(|hunk| {
                            hunk.new_range.contains(&row32) || hunk.new_range.start == row32
                        })
                        .cloned()
                    {
                        cx.emit(EditorInputEvent::HunkClicked {
                            hunk,
                            position: event.position,
                        });
                        return;
                    }
                }
            }
        }
        // ⌥クリック = キャレット追加（multi-cursor・M10-10）。ドラッグ選択は開始しない。
        if event.modifiers.alt {
            self.buffer.add_cursor_at(offset);
            self.marked_range = None;
            self.focus_handle.focus(window, cx);
            cx.notify();
            return;
        }
        self.is_selecting = true;
        // クリック回数で選択粒度を決める（単=キャレット / ダブル=単語 / トリプル以上=行）。
        // ドラッグ拡張（on_mouse_move）も同じ粒度で行うため mode を保持する。
        self.drag_select = if event.modifiers.shift || event.click_count < 2 {
            DragSelectMode::Char
        } else if event.click_count == 2 {
            match self.buffer.snapshot().word_range_at(offset) {
                Some(origin) => DragSelectMode::Word { origin },
                // 単語外（空白・記号）は通常のキャレット配置に流す。
                None => DragSelectMode::Char,
            }
        } else {
            let origin_row = self.buffer.snapshot().byte_to_point(offset).row;
            DragSelectMode::Line { origin_row }
        };
        match self.drag_select.clone() {
            DragSelectMode::Char => {
                if event.modifiers.shift {
                    let anchor = self.primary().anchor;
                    self.buffer
                        .set_selections(vec![Selection::new(anchor, offset)]);
                } else {
                    // 50 行以上のジャンプはナビ履歴へ（⌃- で戻れる・M10-11）。
                    let snapshot = self.buffer.snapshot();
                    let previous = self.primary().head;
                    let distance = snapshot
                        .byte_to_point(previous)
                        .row
                        .abs_diff(snapshot.byte_to_point(offset).row);
                    if distance >= 50 {
                        cx.emit(EditorInputEvent::CaretJumped { from: previous });
                    }
                    self.buffer.set_selections(vec![Selection::cursor(offset)]);
                }
            }
            DragSelectMode::Word { origin } => {
                self.buffer
                    .set_selections(vec![Selection::new(origin.start, origin.end)]);
            }
            DragSelectMode::Line { origin_row } => {
                let range = self.line_selection_range(origin_row);
                self.buffer
                    .set_selections(vec![Selection::new(range.start, range.end)]);
            }
        }
        self.marked_range = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// `row` 行の選択レンジ（末尾の改行を含む。最終行はバッファ末尾まで）。
    /// トリプルクリックとそのドラッグ拡張が使う。
    fn line_selection_range(&self, row: usize) -> Range<usize> {
        let snapshot = self.buffer.snapshot();
        let start = snapshot.point_to_byte(BufferPoint::new(row, 0));
        let end = if row + 1 < snapshot.line_count() {
            snapshot.point_to_byte(BufferPoint::new(row + 1, 0))
        } else {
            snapshot.len_bytes()
        };
        start..end
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
        self.drag_position = None;
    }

    /// ドラッグ中の選択延長（現在の drag_select 粒度で position の位置まで）。
    fn extend_drag_selection(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.offset_for_position(position, window);
        match self.drag_select.clone() {
            DragSelectMode::Char => {
                let anchor = self.primary().anchor;
                self.buffer
                    .set_selections(vec![Selection::new(anchor, offset)]);
            }
            // 単語/行モードはカーソル側も同じ粒度に丸め、起点の単語/行を必ず含める
            // （前方ドラッグは起点 start を anchor に、後方ドラッグは起点 end を anchor に）。
            DragSelectMode::Word { origin } => {
                let at = self
                    .buffer
                    .snapshot()
                    .word_range_at(offset)
                    .unwrap_or(offset..offset);
                let selection = if at.start < origin.start {
                    Selection::new(origin.end, at.start)
                } else {
                    Selection::new(origin.start, at.end.max(origin.end))
                };
                self.buffer.set_selections(vec![selection]);
            }
            DragSelectMode::Line { origin_row } => {
                let row = self.buffer.snapshot().byte_to_point(offset).row;
                let origin = self.line_selection_range(origin_row);
                let at = self.line_selection_range(row);
                let selection = if at.start < origin.start {
                    Selection::new(origin.end, at.start)
                } else {
                    Selection::new(origin.start, at.end.max(origin.end))
                };
                self.buffer.set_selections(vec![selection]);
            }
        }
        cx.notify();
    }

    /// 選択ドラッグ中の move（paint で登録する window 全域ハンドラから）。div の
    /// `on_mouse_move` は hover 中しか発火せず枠外で途切れるため、選択中はこちらが正。
    /// 選択を延長し、ビュー外へはみ出していたら自動スクロールを起動する。
    fn on_drag_selection_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_selecting {
            return;
        }
        self.drag_position = Some(event.position);
        self.extend_drag_selection(event.position, window, cx);
        if self.drag_overshoot(event.position) != 0.0 {
            self.start_drag_autoscroll(window, cx);
        }
    }

    /// ポインタの上下はみ出し量（px）。負 = ビュー上端より上、正 = 下端より下。ビュー内は 0。
    fn drag_overshoot(&self, position: Point<Pixels>) -> f32 {
        let Some(origin) = self.content_origin else {
            return 0.0;
        };
        if self.viewport_height <= px(0.) {
            return 0.0;
        }
        let top = f32::from(origin.y);
        let bottom = top + f32::from(self.viewport_height);
        let y = f32::from(position.y);
        if y < top {
            y - top
        } else if y > bottom {
            y - bottom
        } else {
            0.0
        }
    }

    /// ビュー外ドラッグの自動スクロール tick ループを起動する（既に走っていれば何もしない）。
    fn start_drag_autoscroll(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.drag_autoscroll_running {
            return;
        }
        self.drag_autoscroll_running = true;
        cx.spawn_in(window, async move |editor, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(33))
                    .await;
                let keep_going = editor
                    .update_in(cx, |editor, window, cx| editor.drag_autoscroll_tick(window, cx))
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    /// 自動スクロール 1 tick: はみ出し量に比例して scroll_top を送り、選択端を追従させる。
    /// 続行するなら true（選択終了・ビュー内復帰で自然停止）。
    fn drag_autoscroll_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let overshoot = match self.drag_position {
            Some(position) if self.is_selecting => self.drag_overshoot(position),
            _ => 0.0,
        };
        if overshoot == 0.0 {
            self.drag_autoscroll_running = false;
            return false;
        }
        // 遠くへ引くほど速く（tick あたり最大 48px）。
        let step = overshoot.clamp(-96.0, 96.0) * 0.5;
        self.scroll_top = (self.scroll_top + px(step))
            .max(px(0.))
            .min(self.max_scroll_top());
        if let Some(position) = self.drag_position {
            self.extend_drag_selection(position, window, cx);
        }
        cx.notify();
        true
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 選択ドラッグ中の延長は window 全域ハンドラ（on_drag_selection_move）が担う。
        if self.is_selecting {
            return;
        }
        // hover dwell: 同じ位置に HOVER_DWELL_MS 留まったら Dwell を emit（動くたび世代で無効化）。
        // plain（composer）と無題ファイルは対象外。
        if self.plain || self.buffer.path().is_none() {
            return;
        }
        // 表示中 hover の位置から大きく離れたら Cancel（ポップアップ上は occlude でここへ来ない）。
        if let Some(anchor) = self.last_dwell_anchor {
            let distance = f32::from(
                (event.position.x - anchor.x).abs() + (event.position.y - anchor.y).abs(),
            );
            if distance > HOVER_CANCEL_DISTANCE {
                self.last_dwell_anchor = None;
                cx.emit(EditorHoverEvent::Cancel);
            }
        }
        self.hover_generation = self.hover_generation.wrapping_add(1);
        let generation = self.hover_generation;
        let offset = self.offset_for_position(event.position, window);
        let anchor = event.position;
        self._hover_task = Some(cx.spawn(async move |editor, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(HOVER_DWELL_MS))
                .await;
            let _ = editor.update(cx, |editor, cx| {
                if editor.hover_generation == generation {
                    let (line, character) = editor.lsp_position_for_offset(offset);
                    editor.last_dwell_anchor = Some(anchor);
                    cx.emit(EditorHoverEvent::Dwell {
                        line,
                        character,
                        anchor,
                    });
                }
            });
        }));
    }

    /// hover の dwell/表示を無効化して Cancel を通知する（クリック・スクロールから呼ぶ）。
    fn cancel_hover(&mut self, cx: &mut Context<Self>) {
        self.hover_generation = self.hover_generation.wrapping_add(1);
        if self.last_dwell_anchor.take().is_some() {
            cx.emit(EditorHoverEvent::Cancel);
        }
    }

    fn offset_for_position(&self, position: Point<Pixels>, window: &mut Window) -> usize {
        let Some(origin) = self.content_origin else {
            return self.primary().head;
        };
        let snapshot = self.buffer.snapshot();
        let relative_y = (position.y - origin.y + self.scroll_top).max(px(0.));
        let display_row = ((f32::from(relative_y) / self.line_height_value()).floor() as usize)
            .min(self.total_display_rows(&snapshot).saturating_sub(1));
        // 表示行 → (論理行, セグメント)。マップが古ければ論理行として扱う（次 frame で直る）。
        let (row, segment_range) = if self.wrap_map.key.0 == snapshot.version() {
            let (row, segment) = self.wrap_map.display_to_logical(display_row);
            let line_len = snapshot.line_len_bytes(row);
            (row, self.wrap_map.segment_range(row, segment, line_len))
        } else {
            let row = display_row.min(snapshot.line_count().saturating_sub(1));
            (row, 0..snapshot.line_len_bytes(row))
        };
        let line_text = snapshot.line_text(row);
        let segment_text = line_text
            .get(segment_range.clone())
            .unwrap_or(line_text.as_str())
            .to_string();
        let shaped = shape_plain(&segment_text, self.theme.fg0, self.font_size, window);
        let local_x = (position.x - origin.x).max(px(0.));
        let column = segment_range.start + shaped.closest_index_for_x(local_x);
        snapshot.point_to_byte(BufferPoint::new(row, column))
    }

    /// 括弧/クォートの自動ペア処理。介入したら true（呼び出し側は通常挿入をスキップ）。
    fn handle_pair_input(&mut self, typed: char, cx: &mut Context<Self>) -> bool {
        let selection = self.primary();
        let snapshot = self.buffer.snapshot();
        let previous = if selection.start() > 0 {
            self.buffer
                .text_range(snapshot.prev_char_boundary(selection.start())..selection.start())
                .chars()
                .next()
        } else {
            None
        };
        let next = self
            .buffer
            .text_range(selection.end()..snapshot.next_char_boundary(selection.end()))
            .chars()
            .next();
        match editor_core::classify_pair_input(typed, selection.is_empty(), previous, next) {
            editor_core::PairAction::Insert => false,
            editor_core::PairAction::Pair(close) => {
                let text = format!("{typed}{close}");
                self.buffer.edit(&[selection.range()], &text);
                let cursor = selection.start() + typed.len_utf8();
                self.buffer.set_selections(vec![Selection::cursor(cursor)]);
                self.after_edit(cx);
                cx.emit(EditorInputEvent::Typed(typed.to_string()));
                true
            }
            editor_core::PairAction::Wrap(close) => {
                let inner = self.buffer.text_range(selection.range());
                let text = format!("{typed}{inner}{close}");
                let start = selection.start();
                self.buffer.edit(&[selection.range()], &text);
                // 内側テキストを選択し直す（続けて操作できるように）。
                self.buffer.set_selections(vec![Selection::new(
                    start + typed.len_utf8(),
                    start + typed.len_utf8() + inner.len(),
                )]);
                self.after_edit(cx);
                true
            }
            editor_core::PairAction::SkipOver => {
                let cursor = snapshot.next_char_boundary(selection.end());
                self.buffer.set_selections(vec![Selection::cursor(cursor)]);
                self.blink_visible = true;
                cx.emit(EditorInputEvent::Typed(typed.to_string()));
                cx.notify();
                true
            }
        }
    }

    /// 入力ハンドラの範囲解決: 明示レンジ → marked → 現在選択、の順（すべて byte）。
    fn resolve_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        if let Some(range) = range_utf16 {
            self.buffer.utf16_to_byte(range.start)..self.buffer.utf16_to_byte(range.end)
        } else if let Some(marked) = self.marked_range.clone() {
            marked
        } else {
            self.primary().range()
        }
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// composer は親（agent_panel）へ [`ComposerEvent`] を通知できる（Enter 送信の委譲）。
impl EventEmitter<ComposerEvent> for EditorView {}

/// 確定入力の通知（workspace が補完の自動トリガに使う）。
impl EventEmitter<EditorInputEvent> for EditorView {}

/// hover dwell の通知（workspace が LSP hover 要求に使う）。
impl EventEmitter<EditorHoverEvent> for EditorView {}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `.md` 整形プレビュー（rendered 側）: EditorElement を差し替え、編集ハンドラ/マウスは載せない。
        // ブロックは version キーでキャッシュ（idle 再描画で再パースしない）。
        if self.rendered_markdown && self.is_markdown() {
            let version = self.buffer.version();
            if self.markdown_blocks_version != version {
                self.markdown_blocks = markdown::parse(&self.buffer.text());
                self.markdown_blocks_version = version;
            }
            return div()
                .key_context("Editor")
                .track_focus(&self.focus_handle(cx))
                .size_full()
                .bg(self.theme.bg1)
                .text_color(self.theme.fg0)
                .font_family("IBM Plex Sans JP")
                .on_action(cx.listener(Self::toggle_rendered_markdown))
                .child(markdown_preview::render_preview(
                    &self.markdown_blocks,
                    &self.theme,
                    self.font_size,
                    &self.markdown_scroll,
                    self.buffer.path().and_then(|path| path.parent()),
                ))
                .into_any_element();
        }
        div()
            .key_context("Editor")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .when(!self.plain, |element| element.bg(self.theme.bg1))
            .text_color(self.theme.fg0)
            // コード = Guguru Sans Code（等幅・bin で bundle 済み）/ composer(plain) = IBM Plex Sans JP（UI）
            .font_family(if self.plain {
                "IBM Plex Sans JP"
            } else {
                "Guguru Sans Code"
            })
            .text_size(px(self.font_size))
            .line_height(px(self.line_height_value()))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::move_word_left))
            .on_action(cx.listener(Self::move_word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_backward))
            .on_action(cx.listener(Self::move_to_start))
            .on_action(cx.listener(Self::move_to_end))
            .on_action(cx.listener(Self::move_line_up))
            .on_action(cx.listener(Self::move_line_down))
            .on_action(cx.listener(Self::duplicate_line_up))
            .on_action(cx.listener(Self::duplicate_line_down))
            .on_action(cx.listener(Self::delete_line))
            .on_action(cx.listener(Self::toggle_comment))
            .on_action(cx.listener(Self::tab_indent))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::add_cursor_above))
            .on_action(cx.listener(Self::add_cursor_below))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::toggle_soft_wrap))
            .on_action(cx.listener(Self::toggle_rendered_markdown))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_to_line_start))
            .on_action(cx.listener(Self::move_to_line_end))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::save))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(EditorElement {
                editor: cx.entity(),
            })
            .into_any_element()
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let start = self.buffer.utf16_to_byte(range_utf16.start);
        let end = self.buffer.utf16_to_byte(range_utf16.end);
        actual_range.replace(self.buffer.byte_to_utf16(start)..self.buffer.byte_to_utf16(end));
        Some(self.buffer.text_range(start..end))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.primary();
        Some(UTF16Selection {
            range: self.buffer.byte_to_utf16(selection.start())
                ..self.buffer.byte_to_utf16(selection.end()),
            reversed: selection.head < selection.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let marked = self.marked_range.as_ref()?;
        Some(self.buffer.byte_to_utf16(marked.start)..self.buffer.byte_to_utf16(marked.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 括弧/クォートの自動ペア（M10-9）。通常タイプ（明示レンジ無し・非 IME・コードのみ）だけ介入する。
        if !self.plain && range_utf16.is_none() && self.marked_range.is_none() {
            let mut chars = new_text.chars();
            if let (Some(typed), None) = (chars.next(), chars.next()) {
                if self.handle_pair_input(typed, cx) {
                    return;
                }
            }
        }
        let range = self.resolve_range(range_utf16);
        self.buffer.edit(&[range], new_text);
        self.marked_range = None;
        self.refresh_highlights();
        self.pending_caret_reveal = true;
        // 確定入力を親へ通知（補完の自動トリガ）。IME 変換中（replace_and_mark）は通らない経路。
        if !new_text.is_empty() {
            cx.emit(EditorInputEvent::Typed(new_text.to_string()));
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.resolve_range(range_utf16);
        let start = range.start;
        self.buffer.edit(&[range], new_text);

        self.marked_range = if new_text.is_empty() {
            None
        } else {
            Some(start..start + new_text.len())
        };

        let selection = match new_selected_range_utf16 {
            Some(selected) => Selection::new(
                start + utf16_to_byte_in(new_text, selected.start),
                start + utf16_to_byte_in(new_text, selected.end),
            ),
            None => Selection::cursor(start + new_text.len()),
        };
        self.buffer.set_selections(vec![selection]);
        self.refresh_highlights();
        self.pending_caret_reveal = true;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 変換候補ウィンドウはキャレット位置に出す（M2 の近似）。
        self.caret_bounds
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let byte = self.offset_for_position(point, window);
        Some(self.buffer.byte_to_utf16(byte))
    }
}

// ── カスタム Element（描画本体） ──

struct EditorElement {
    editor: Entity<EditorView>,
}

struct PositionedLine {
    line: ShapedLine,
    origin: Point<Pixels>,
}

struct EditorPrepaint {
    line_numbers: Vec<PositionedLine>,
    text_lines: Vec<PositionedLine>,
    selections: Vec<PaintQuad>,
    carets: Vec<PaintQuad>,
    current_line: Option<PaintQuad>,
    diff_marks: Vec<PaintQuad>,
    diagnostic_marks: Vec<PaintQuad>,
    search_marks: Vec<PaintQuad>,
    content_bounds: Bounds<Pixels>,
    gutter_width: Pixels,
    viewport_height: Pixels,
    caret_bounds: Option<Bounds<Pixels>>,
    focused: bool,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = EditorPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // wrap 列数の材料（等幅セル幅・ガター概算）を先に測る（soft wrap・M10-12）。
        // コード（等幅 Guguru Sans Code）は 'M' の advance が全グリフ共通なのでそれがセル幅。
        let editor_font_size = self.editor.read(cx).font_size;
        let cell_width_estimate =
            f32::from(shape_plain("M", gpui::black(), editor_font_size, window).width).max(1.0);
        // composer（プロポーショナルな Sans）は「等幅セル」が無い。折返しは半角=1 / 全角=2 セルで
        // 数える（compute_wrap_segments）ので、セル幅の単位 = 全角グリフ幅 ÷2（＝半角1個分）に採る。
        // これで全角の折返しは画素ぴったり・半角も平均字幅に近く、右端まで使い切る。
        // （旧: 'M' 幅を単位 → Sans では過大評価で ~3 割手前の早折れ・右に余白が残っていた）
        let composer_cell_estimate =
            (f32::from(shape_plain("永", gpui::black(), editor_font_size, window).width) / 2.0)
                .max(1.0);
        // 保留リビール（検索ジャンプ等）を viewport 確定後の今、消化する（対象行を中央へ寄せる）。
        // 併せて wrap マップの再構築・gutter diff の再計算（いずれも変化時のみ）。
        self.editor.update(cx, |view, cx| {
            let snapshot = view.buffer.snapshot();
            // wrap マップ: (version, columns, on/off) が変わった時だけ作り直す。
            // plain（composer）は**常に折り返し**（長文が右へはみ出さない・横スクロールは無い）。
            let wrap_on = view.soft_wrap || view.plain;
            let columns = if wrap_on {
                if view.plain {
                    // composer はガター無し。半角セル幅（全角÷2）を単位に折返し桁数を出す。
                    ((f32::from(bounds.size.width) / composer_cell_estimate).floor() as usize)
                        .max(10)
                } else {
                    let digits = snapshot.line_count().to_string().len() as f32;
                    let gutter =
                        (digits * cell_width_estimate + GUTTER_PADDING * 2.0).max(GUTTER_MIN_WIDTH);
                    (((f32::from(bounds.size.width) - gutter) / cell_width_estimate).floor()
                        as usize)
                        .max(10)
                }
            } else {
                0
            };
            let key = (snapshot.version(), columns, wrap_on);
            if view.wrap_map.key != key {
                view.wrap_map = if key.2 {
                    WrapMap::build(&snapshot, columns, key)
                } else {
                    WrapMap::identity(snapshot.line_count(), key)
                };
            }
            if let Some(offset) = view.pending_reveal.take() {
                let line_height = view.line_height_value();
                let row = view.display_row_for_offset(&snapshot, offset) as f32;
                let viewport = f32::from(bounds.size.height);
                let row_top = row * line_height;
                let current_top = f32::from(view.scroll_top);
                // 既に丸ごと見えている行へは動かさない（⌘F のインクリメンタル巡回で画面が跳ねない）。
                // 見えていなければ中央へ。
                let fully_visible =
                    row_top >= current_top && row_top + line_height <= current_top + viewport;
                if !fully_visible {
                    let total = view.total_display_rows(&snapshot) as f32 * line_height;
                    let target = row_top - viewport / 2.0 + line_height / 2.0;
                    view.scroll_top = px(target.clamp(0.0, (total - viewport).max(0.0)));
                }
            }
            // 編集/カーソル移動が要求した caret 可視化を**今**（wrap_map 再構築後）に消化する。
            // viewport 未確定（初回描画前）の間は短絡でフラグを保持し、確定後の frame で reveal する。
            if view.viewport_height > px(0.) && std::mem::take(&mut view.pending_caret_reveal) {
                view.scroll_caret_into_view();
            }
            if view.buffer.version() != view.diff_scheduled_version {
                view.diff_scheduled_version = view.buffer.version();
                view.schedule_diff(cx);
            }
            // ハイライト Tree の追従（増分 or 全文・M11-8）。
            view.sync_highlight_tree();
        });
        let view = self.editor.read(cx);
        let snapshot = view.buffer.snapshot();
        let theme = view.theme.clone();
        let accent = view.accent;
        let scroll_top = view.scroll_top;
        let marked = view.marked_range.clone();
        let focused = view.focus_handle.is_focused(window);
        let selections = view.buffer.selections().to_vec();

        let diff_hunks = view.diff_hunks.clone();
        let diagnostics = view.diagnostics.clone();
        let search_ranges = view.search_ranges.clone();
        let line_annotation = view.line_annotation.clone();
        let agent_mark_color = view.agent_mark_color;
        let plain = view.plain;
        let blink_visible = view.blink_visible;
        let caret_blink_enabled = view.caret_blink_enabled;
        let text_font = window.text_style().font();
        let primary = selections.first().copied().unwrap_or(Selection::cursor(0));

        let line_count = snapshot.line_count();
        let font_size = view.font_size;
        let line_height_value = view.line_height_value();
        let line_height = px(line_height_value);

        // ガター幅: 最大行番号を shape して測る（平坦モードはガター無し）。
        let gutter_width = if plain {
            px(0.)
        } else {
            let widest_number = line_count.to_string();
            let sample = shape_plain(&widest_number, theme.fg2, font_size, window);
            px((f32::from(sample.width) + GUTTER_PADDING * 2.0).max(GUTTER_MIN_WIDTH))
        };

        let content_origin = point(bounds.left() + gutter_width, bounds.top());
        let content_width = (bounds.size.width - gutter_width).max(px(0.));
        let content_bounds = Bounds::new(content_origin, size(content_width, bounds.size.height));

        // 仮想化は**表示行**で回す（wrap off は恒等マップ = 従来と同一・M10-12）。
        let wrap_map = &self.editor.read(cx).wrap_map;
        let total_display = wrap_map.total_display_rows();
        let visible = visible_rows(
            f32::from(scroll_top),
            f32::from(bounds.size.height),
            line_height_value,
            total_display,
        );

        // 可視表示行のセグメントが跨る byte 範囲だけハイライトを問い合わせる（M11-8）。
        let highlights: Vec<lang::HighlightSpan> = if visible.is_empty() {
            Vec::new()
        } else {
            let (first_line, first_segment) = wrap_map.display_to_logical(visible.start);
            let (last_line, last_segment) =
                wrap_map.display_to_logical(visible.end.saturating_sub(1));
            let start_byte = snapshot.point_to_byte(BufferPoint::new(first_line, 0))
                + wrap_map
                    .segment_range(
                        first_line,
                        first_segment,
                        snapshot.line_len_bytes(first_line),
                    )
                    .start;
            let end_byte = snapshot.point_to_byte(BufferPoint::new(last_line, 0))
                + wrap_map
                    .segment_range(last_line, last_segment, snapshot.line_len_bytes(last_line))
                    .end;
            view.highlighter
                .as_ref()
                .map(|highlighter| {
                    highlighter.spans(&snapshot.text(), start_byte..end_byte.max(start_byte))
                })
                .unwrap_or_default()
        };

        let mut line_numbers = Vec::new();
        let mut text_lines = Vec::new();
        let mut selection_quads = Vec::new();
        let mut carets = Vec::new();
        let mut current_line = None;
        let mut caret_bounds = None;
        let mut diff_marks = Vec::new();
        let mut diagnostic_marks = Vec::new();
        let mut search_marks = Vec::new();

        let primary_point = snapshot.byte_to_point(primary.head);
        let primary_display_row =
            wrap_map.logical_to_display(primary_point.row, primary_point.column);

        for display_row in visible {
            let (row, segment) = wrap_map.display_to_logical(display_row);
            let y = bounds.top() + line_height * (display_row as f32) - scroll_top;
            let is_first_segment = segment == 0;

            // gutter diff マーク（左端の細いバー）。plain（composer）は gutter が無いので出さない。
            // wrap 中は論理行の先頭セグメントのみ。
            if !plain && is_first_segment {
                let row32 = row as u32;
                for hunk in &diff_hunks {
                    match hunk.kind {
                        project::HunkKind::Added | project::HunkKind::Modified
                            if hunk.new_range.contains(&row32) =>
                        {
                            // エージェント編集のファイルはスレッド色で（生中継の帰属表示・M12-3）。
                            let color = agent_mark_color.unwrap_or(
                                if hunk.kind == project::HunkKind::Added {
                                    theme.ok
                                } else {
                                    theme.warn
                                },
                            );
                            diff_marks.push(fill(
                                Bounds::new(
                                    point(bounds.left(), y + px(1.)),
                                    size(px(2.5), line_height - px(2.)),
                                ),
                                color,
                            ));
                        }
                        // 削除は行 row の上境界に小さな err マーカー。
                        project::HunkKind::Removed if hunk.new_range.start == row32 => {
                            diff_marks.push(fill(
                                Bounds::new(
                                    point(bounds.left(), y - px(1.5)),
                                    size(px(6.), px(3.)),
                                ),
                                theme.err,
                            ));
                        }
                        _ => {}
                    }
                }
            }

            // 診断の下線（error=赤 / warn=琥珀 / info・hint=ミュート）。行のテキスト幅いっぱいに引く。
            if !plain && is_first_segment {
                let row32 = row as u32;
                if let Some((_, severity)) = diagnostics.iter().find(|(line, _)| *line == row32) {
                    let color = match severity {
                        lang::lsp::Severity::Error => theme.err,
                        lang::lsp::Severity::Warning => theme.warn,
                        _ => theme.fg2,
                    };
                    diagnostic_marks.push(fill(
                        Bounds::new(
                            point(content_origin.x, y + line_height - px(2.)),
                            size(content_width, px(1.5)),
                        ),
                        color,
                    ));
                }
            }

            let full_line_text = snapshot.line_text(row);
            let segment_range = wrap_map.segment_range(row, segment, full_line_text.len());
            let line_start = snapshot.point_to_byte(BufferPoint::new(row, 0)) + segment_range.start;
            let line_text = full_line_text[segment_range.clone()].to_string();
            let line_end = line_start + line_text.len();
            // キャレットがこのセグメントに乗るか（セグメント境界の offset は次セグメント側。
            // 論理行末だけは最終セグメントに乗せる）。
            let is_last_segment = segment_range.end == full_line_text.len();

            if !plain && display_row == primary_display_row {
                current_line = Some(fill(
                    Bounds::new(point(content_origin.x, y), size(content_width, line_height)),
                    hsla(0., 0., 1., 0.045),
                ));
            }

            // 行番号（右寄せ・平坦モードは無し・wrap 中は先頭セグメントのみ）
            if !plain && is_first_segment {
                let number_text = (row + 1).to_string();
                let number_line = shape_plain(&number_text, theme.fg2, font_size, window);
                let number_x =
                    bounds.left() + gutter_width - px(GUTTER_PADDING) - number_line.width;
                line_numbers.push(PositionedLine {
                    line: number_line,
                    origin: point(number_x, y),
                });
            }

            // 本文（構文ハイライト + IME 変換中の下線）
            let line_spans = lang::spans_in_range(&highlights, line_start, line_end);
            let runs = build_line_runs(
                &line_text,
                line_start,
                line_spans,
                marked.clone(),
                theme.fg0,
                &theme.syntax,
                &text_font,
            );
            let shaped = window.text_system().shape_line(
                SharedString::from(line_text.clone()),
                px(font_size),
                &runs,
                None,
            );

            // 検索ハイライト（⌘F の全マッチ）。選択面より先に paint する＝下に敷く。
            for range in ranges_overlapping_line(&search_ranges, line_start, line_end) {
                if range.start == range.end {
                    continue; // 零幅（正規表現の空マッチ）は塗らない
                }
                let visible_start = range.start.max(line_start) - line_start;
                let x_start = content_origin.x + shaped.x_for_index(visible_start);
                let x_end = if range.end > line_end {
                    content_origin.x + content_width
                } else {
                    content_origin.x + shaped.x_for_index(range.end - line_start)
                };
                if x_end > x_start {
                    search_marks.push(fill(
                        Bounds::from_corners(point(x_start, y), point(x_end, y + line_height)),
                        theme.warn.alpha(0.16),
                    ));
                }
            }

            // 選択面
            for selection in &selections {
                if selection.is_empty()
                    || selection.end() <= line_start
                    || selection.start() > line_end
                {
                    continue;
                }
                let visible_start = selection.start().max(line_start) - line_start;
                let x_start = content_origin.x + shaped.x_for_index(visible_start);
                let x_end = if selection.end() > line_end {
                    content_origin.x + content_width
                } else {
                    content_origin.x + shaped.x_for_index(selection.end() - line_start)
                };
                if x_end > x_start {
                    selection_quads.push(fill(
                        Bounds::from_corners(point(x_start, y), point(x_end, y + line_height)),
                        theme_core::editor_selection(),
                    ));
                }
            }

            // キャレット（空選択）。境界 offset は次セグメント側・論理行末は最終セグメント。
            for (index, selection) in selections.iter().enumerate() {
                let head = selection.head;
                let on_segment = head >= line_start
                    && (head < line_end || (head == line_end && is_last_segment));
                if !selection.is_empty() || !on_segment {
                    continue;
                }
                let caret_x = content_origin.x + shaped.x_for_index(selection.head - line_start);
                let caret_rect = Bounds::new(point(caret_x, y), size(px(2.), line_height));
                carets.push(fill(caret_rect, accent));
                if index == 0 {
                    caret_bounds = Some(caret_rect);
                }
            }

            // blame 注釈（キャレット行の行末に dim・最終セグメントのみ・M11-11）。
            if let Some((annotation_row, annotation_text)) = &line_annotation {
                if *annotation_row == row && is_last_segment && !plain {
                    let annotation = shape_plain(
                        annotation_text,
                        theme.fg2.alpha(0.7),
                        font_size * 0.9,
                        window,
                    );
                    let x = content_origin.x + shaped.width + px(32.);
                    if x + annotation.width < content_origin.x + content_width {
                        text_lines.push(PositionedLine {
                            line: annotation,
                            origin: point(x, y),
                        });
                    }
                }
            }

            text_lines.push(PositionedLine {
                line: shaped,
                origin: point(content_origin.x, y),
            });
        }

        // focus 中かつ点滅 ON のフレームだけキャレットを出す。
        if !focused || (caret_blink_enabled && !blink_visible) {
            carets.clear();
        }

        EditorPrepaint {
            line_numbers,
            text_lines,
            selections: selection_quads,
            carets,
            current_line,
            diff_marks,
            diagnostic_marks,
            search_marks,
            content_bounds,
            gutter_width,
            viewport_height: bounds.size.height,
            caret_bounds,
            focused,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(prepaint.content_bounds, self.editor.clone()),
            cx,
        );

        // 選択ドラッグの move は window 全域で拾う（div の on_mouse_move は hover 中しか
        // 発火せず、枠外へ引っ張ると途切れるため）。is_selecting のビューだけが反応する。
        {
            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.pressed_button != Some(MouseButton::Left)
                {
                    return;
                }
                editor.update(cx, |editor, cx| {
                    editor.on_drag_selection_move(event, window, cx)
                });
            });
        }

        if let Some(highlight) = prepaint.current_line.take() {
            window.paint_quad(highlight);
        }
        // gutter diff マーク（追加=緑 / 変更=琥珀 / 削除=赤の左端バー）。
        for mark in prepaint.diff_marks.drain(..) {
            window.paint_quad(mark);
        }
        // 検索ハイライト（⌘F 全マッチ）は選択面の下に敷く。
        for mark in prepaint.search_marks.drain(..) {
            window.paint_quad(mark);
        }
        for quad in prepaint.selections.drain(..) {
            window.paint_quad(quad);
        }
        let line_height = px(self.editor.read(cx).line_height_value());
        for positioned in prepaint.line_numbers.drain(..) {
            if let Err(error) = positioned.line.paint(
                positioned.origin,
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            ) {
                eprintln!("gutter paint 失敗: {error}");
            }
        }
        for positioned in prepaint.text_lines.drain(..) {
            if let Err(error) = positioned.line.paint(
                positioned.origin,
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            ) {
                eprintln!("text paint 失敗: {error}");
            }
        }
        // 診断の下線はテキストの上に重ねる（error=赤 等）。
        for mark in prepaint.diagnostic_marks.drain(..) {
            window.paint_quad(mark);
        }
        for caret in prepaint.carets.drain(..) {
            window.paint_quad(caret);
        }

        let content_origin = prepaint.content_bounds.origin;
        let viewport_height = prepaint.viewport_height;
        let gutter_width = prepaint.gutter_width;
        let caret_bounds = prepaint.caret_bounds;
        let focused = prepaint.focused;
        self.editor.update(cx, |view, cx| {
            view.content_origin = Some(content_origin);
            view.viewport_height = viewport_height;
            view.gutter_width = gutter_width;
            view.caret_bounds = caret_bounds;
            // 点滅の start/stop を focus・window activation・親の許可へ連動させる。
            // 停止時は blink_visible=true に戻し、キャレットを静止表示する。
            let should_blink = focused && window.is_window_active() && view.caret_blink_enabled;
            if should_blink {
                if view._blink_task.is_none() {
                    view.start_blinking(cx);
                }
            } else if view._blink_task.is_some() {
                view._blink_task = None;
                view.blink_visible = true;
            }
        });
    }
}

// ── 自由関数 ──

/// 表示すべき行レンジ（仮想化）。scroll_top と viewport から見える行だけを返す。
fn visible_rows(
    scroll_top: f32,
    viewport_height: f32,
    line_height: f32,
    line_count: usize,
) -> Range<usize> {
    if line_height <= 0.0 {
        return 0..line_count;
    }
    let first = (scroll_top / line_height).floor().max(0.0) as usize;
    let last = ((scroll_top + viewport_height) / line_height).ceil() as usize;
    first.min(line_count)..last.min(line_count)
}

/// `ranges`（start 昇順・非重複）のうち `start..=end` に重なる部分列を二分探索で切り出す。
/// 行ごとの検索ハイライト描画で使う（マッチが多くても可視行分しか見ない）。
fn ranges_overlapping_line(ranges: &[Range<usize>], start: usize, end: usize) -> &[Range<usize>] {
    let first = ranges.partition_point(|range| range.end < start);
    let mut last = first;
    while last < ranges.len() && ranges[last].start <= end {
        last += 1;
    }
    &ranges[first..last]
}

/// 縦移動（row ± delta、列は据え置き。行末を超えたら行末にクリップ）。
fn vertical(snapshot: &BufferSnapshot, head: usize, delta: isize) -> usize {
    let point = snapshot.byte_to_point(head);
    let target_row = (point.row as isize + delta).max(0) as usize;
    let target_row = target_row.min(snapshot.line_count().saturating_sub(1));
    snapshot.point_to_byte(BufferPoint::new(target_row, point.column))
}

/// 行頭 / 行末の byte offset。
fn line_edge(snapshot: &BufferSnapshot, head: usize, end: bool) -> usize {
    let row = snapshot.byte_to_point(head).row;
    let column = if end { usize::MAX } else { 0 };
    snapshot.point_to_byte(BufferPoint::new(row, column))
}

/// 行の TextRun 列を組む。構文ハイライト色（[`lang::HighlightSpan`]）+ IME 変換中の下線を合成する。
/// `line_spans` はこの行に重なる span（非重複・start 昇順）。連続する同スタイルは 1 run に連結。
fn build_line_runs(
    text: &str,
    line_start: usize,
    line_spans: &[lang::HighlightSpan],
    marked: Option<Range<usize>>,
    default_color: gpui::Hsla,
    syntax: &SyntaxColors,
    text_font: &gpui::Font,
) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut byte = 0usize;
    for character in text.chars() {
        let absolute = line_start + byte;
        let kind = line_spans
            .iter()
            .find(|span| span.range.start <= absolute && absolute < span.range.end)
            .map(|span| span.kind);
        let color = kind
            .map(|kind| syntax_color(kind, syntax))
            .unwrap_or(default_color);
        let mut font = text_font.clone();
        match kind {
            Some(lang::HighlightKind::Heading | lang::HighlightKind::Strong) => {
                font.weight = gpui::FontWeight::BOLD;
            }
            Some(lang::HighlightKind::Emphasis) => {
                font.style = gpui::FontStyle::Italic;
            }
            _ => {}
        }
        let background_color =
            matches!(kind, Some(lang::HighlightKind::Code)).then(|| syntax.string.alpha(0.10));
        let underlined = marked
            .as_ref()
            .is_some_and(|range| absolute >= range.start && absolute < range.end);
        let underline = if underlined {
            Some(UnderlineStyle {
                color: Some(color),
                thickness: px(1.0),
                wavy: false,
            })
        } else if matches!(kind, Some(lang::HighlightKind::Link)) {
            Some(UnderlineStyle {
                color: Some(color),
                thickness: px(1.0),
                wavy: false,
            })
        } else {
            None
        };
        let char_len = character.len_utf8();
        match runs.last_mut() {
            Some(run)
                if run.color == color
                    && run.font == font
                    && run.background_color == background_color
                    && run.underline == underline =>
            {
                run.len += char_len;
            }
            _ => runs.push(TextRun {
                len: char_len,
                font,
                color,
                background_color,
                underline,
                strikethrough: None,
            }),
        }
        byte += char_len;
    }
    runs
}

/// [`lang::HighlightKind`] を theme の syn-* 色にマップする（UI-SPEC §1.1）。
fn syntax_color(kind: lang::HighlightKind, syntax: &SyntaxColors) -> gpui::Hsla {
    match kind {
        lang::HighlightKind::Keyword => syntax.keyword,
        lang::HighlightKind::Function => syntax.function,
        lang::HighlightKind::Type => syntax.type_,
        lang::HighlightKind::String => syntax.string,
        lang::HighlightKind::Number => syntax.number,
        lang::HighlightKind::Comment => syntax.comment,
        lang::HighlightKind::Macro => syntax.macro_,
        lang::HighlightKind::Punctuation => syntax.punctuation,
        lang::HighlightKind::Heading => syntax.keyword,
        lang::HighlightKind::Link => syntax.function,
        lang::HighlightKind::Strong => syntax.type_,
        lang::HighlightKind::Emphasis => syntax.macro_,
        lang::HighlightKind::Code => syntax.string,
    }
}

// ── soft wrap（M10-12）: 論理行→表示行マップ。設計は docs/JOURNAL.md 2026-07-16 ──

/// 1 論理行をセル幅 `columns` で折り返したときの**セグメント開始 byte 列**（先頭は必ず 0）。
/// 等幅前提: ASCII=1 セル・東アジア全角=2 セル（unicode-width）。単語境界（空白の直後）優先で切り、
/// 1 語が幅を超えるときは文字で切る。columns==0 は折返し無し扱い。
fn compute_wrap_segments(line: &str, columns: usize) -> Vec<usize> {
    use unicode_width::UnicodeWidthChar as _;
    let mut starts = vec![0usize];
    if columns == 0 {
        return starts;
    }
    let mut cells = 0usize;
    let mut last_break: Option<usize> = None; // 直近の折返し候補（空白の直後の byte）
    let mut segment_start = 0usize;
    let mut byte = 0usize;
    for c in line.chars() {
        let width = c.width().unwrap_or(1).max(1);
        if cells + width > columns && byte > segment_start {
            // 単語境界があればそこで、無ければ現在位置（文字境界）で折る。
            let break_at = match last_break {
                Some(candidate) if candidate > segment_start => candidate,
                _ => byte,
            };
            starts.push(break_at);
            segment_start = break_at;
            // 折返し後のセル数を数え直す（break_at..byte+この文字）。
            cells = line[break_at..byte]
                .chars()
                .map(|c| c.width().unwrap_or(1).max(1))
                .sum();
            last_break = None;
        }
        cells += width;
        byte += c.len_utf8();
        if c == ' ' || c == '\t' {
            last_break = Some(byte);
        }
    }
    starts
}

/// 論理行↔表示行のマップ。(buffer version, columns, 有効) が変わったら作り直す（prepaint）。
struct WrapMap {
    /// 論理行ごとのセグメント開始 byte 列。無効時や短い行は `[0]`。
    segments: Vec<Vec<usize>>,
    /// 論理行 i の先頭表示行（累積）。len = 行数 + 1・末尾 = 総表示行数。
    prefix_rows: Vec<usize>,
    /// このマップを計算した (buffer version, columns, enabled)。
    key: (u64, usize, bool),
}

impl WrapMap {
    fn identity(line_count: usize, key: (u64, usize, bool)) -> WrapMap {
        WrapMap {
            segments: vec![vec![0]; line_count],
            prefix_rows: (0..=line_count).collect(),
            key,
        }
    }

    fn build(snapshot: &BufferSnapshot, columns: usize, key: (u64, usize, bool)) -> WrapMap {
        let line_count = snapshot.line_count();
        let mut segments = Vec::with_capacity(line_count);
        let mut prefix_rows = Vec::with_capacity(line_count + 1);
        let mut total = 0usize;
        for row in 0..line_count {
            prefix_rows.push(total);
            let starts = compute_wrap_segments(&snapshot.line_text(row), columns);
            total += starts.len();
            segments.push(starts);
        }
        prefix_rows.push(total);
        WrapMap {
            segments,
            prefix_rows,
            key,
        }
    }

    fn total_display_rows(&self) -> usize {
        self.prefix_rows.last().copied().unwrap_or(0).max(1)
    }

    /// 表示行 → (論理行, セグメント番号)。
    fn display_to_logical(&self, display_row: usize) -> (usize, usize) {
        let line = self
            .prefix_rows
            .partition_point(|&first| first <= display_row)
            .saturating_sub(1)
            .min(self.segments.len().saturating_sub(1));
        let segment = display_row.saturating_sub(self.prefix_rows[line]);
        (
            line,
            segment.min(self.segments[line].len().saturating_sub(1)),
        )
    }

    /// (論理行, 行内 byte 列) → 表示行。
    fn logical_to_display(&self, line: usize, column: usize) -> usize {
        let line = line.min(self.segments.len().saturating_sub(1));
        let starts = &self.segments[line];
        let segment = starts
            .partition_point(|&start| start <= column)
            .saturating_sub(1);
        self.prefix_rows[line] + segment
    }

    /// 表示行のセグメント byte 範囲（行内相対）。end は行 byte 長（呼び出し側が渡す）でクリップ。
    fn segment_range(&self, line: usize, segment: usize, line_len: usize) -> Range<usize> {
        let starts = &self.segments[line.min(self.segments.len().saturating_sub(1))];
        let start = starts.get(segment).copied().unwrap_or(0);
        let end = starts.get(segment + 1).copied().unwrap_or(line_len);
        start..end.max(start)
    }
}

/// 単色 1 行を shape する（行番号・ヒットテスト用）。フォントはウィンドウの解決済みフォントを使う。
fn shape_plain(text: &str, color: gpui::Hsla, font_size: f32, window: &mut Window) -> ShapedLine {
    let text_font = window.text_style().font();
    let run = TextRun {
        len: text.len(),
        font: text_font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(font_size),
        &[run],
        None,
    )
}

/// new_text 内の UTF-16 offset を byte offset に変換する（変換中選択の解決）。
fn utf16_to_byte_in(text: &str, utf16: usize) -> usize {
    let mut counted_utf16 = 0;
    let mut byte = 0;
    for ch in text.chars() {
        if counted_utf16 >= utf16 {
            break;
        }
        counted_utf16 += ch.len_utf16();
        byte += ch.len_utf8();
    }
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_rows_covers_only_the_window() {
        // 23px 行・viewport 230px はちょうど 10 行 → 0..10
        assert_eq!(visible_rows(0.0, 230.0, 23.0, 100), 0..10);
        // 半端な高さは下端の 1 行を含める（ceil）
        assert_eq!(visible_rows(0.0, 240.0, 23.0, 100), 0..11);
        // 230px スクロール → 10 行目から、10..20
        assert_eq!(visible_rows(230.0, 230.0, 23.0, 100), 10..20);
        // 行数を超えない（末尾クランプ）
        assert_eq!(visible_rows(0.0, 230.0, 23.0, 3), 0..3);
    }

    #[test]
    fn ranges_overlapping_line_slices_by_binary_search() {
        let ranges = vec![0..3, 5..8, 10..20, 25..26];
        // 行 [4, 9]: 5..8 のみ（10..20 は次の行から始まる）
        assert_eq!(ranges_overlapping_line(&ranges, 4, 9), &[5..8]);
        // 行 [12, 15]: 跨いでいる 10..20 のみ
        assert_eq!(ranges_overlapping_line(&ranges, 12, 15), &[10..20]);
        // 行 [21, 24]: どれとも重ならない
        assert!(ranges_overlapping_line(&ranges, 21, 24).is_empty());
        // 端の一致（end == 行頭 / start == 行末）も含む
        assert_eq!(ranges_overlapping_line(&ranges, 3, 5), &[0..3, 5..8]);
        // 空列
        assert!(ranges_overlapping_line(&[], 0, 100).is_empty());
    }

    #[test]
    fn wrap_segments_break_at_word_boundaries_and_cjk_cells() {
        // 10 セル幅・"hello world foo" → "hello " / "world foo"（語境界優先・後半 9 セルは収まる）
        assert_eq!(compute_wrap_segments("hello world foo", 10), vec![0, 6]);
        // 8 セル幅なら 3 段: "hello " / "world " / "foo"
        assert_eq!(compute_wrap_segments("hello world foo", 8), vec![0, 6, 12]);
        // 単語が幅を超える → 文字で切る
        assert_eq!(compute_wrap_segments("aaaaaaaaaaaa", 5), vec![0, 5, 10]);
        // 全角は 2 セル: 4 セル幅に「ああah」→「ああ」(4) / 「ah」
        assert_eq!(compute_wrap_segments("ああah", 4), vec![0, 6]);
        // 幅内は折り返さない・columns=0 は無効
        assert_eq!(compute_wrap_segments("short", 80), vec![0]);
        assert_eq!(compute_wrap_segments("anything at all", 0), vec![0]);
        // 空行
        assert_eq!(compute_wrap_segments("", 8), vec![0]);
    }

    #[test]
    fn wrap_map_round_trips_display_and_logical_rows() {
        let buffer = Buffer::from_str("short\naaaaaaaaaaaa\nx");
        let snapshot = buffer.snapshot();
        let map = WrapMap::build(&snapshot, 5, (0, 5, true));
        // 行0=1seg・行1=3seg(12 文字/5)・行2=1seg → 計 5 表示行
        assert_eq!(map.total_display_rows(), 5);
        assert_eq!(map.display_to_logical(0), (0, 0));
        assert_eq!(map.display_to_logical(1), (1, 0));
        assert_eq!(map.display_to_logical(2), (1, 1));
        assert_eq!(map.display_to_logical(3), (1, 2));
        assert_eq!(map.display_to_logical(4), (2, 0));
        // 逆写像
        assert_eq!(map.logical_to_display(1, 0), 1);
        assert_eq!(map.logical_to_display(1, 5), 2);
        assert_eq!(map.logical_to_display(1, 11), 3);
        // セグメント範囲
        assert_eq!(map.segment_range(1, 1, 12), 5..10);
        assert_eq!(map.segment_range(1, 2, 12), 10..12);
        // 恒等マップ
        let identity = WrapMap::identity(3, (0, 0, false));
        assert_eq!(identity.total_display_rows(), 3);
        assert_eq!(identity.display_to_logical(2), (2, 0));
    }

    #[test]
    fn utf16_to_byte_in_handles_japanese_and_astral() {
        assert_eq!(utf16_to_byte_in("あい", 0), 0);
        assert_eq!(utf16_to_byte_in("あい", 1), 3); // "あ" = 3 バイト
        assert_eq!(utf16_to_byte_in("あい", 2), 6);
        // "𝔸" は UTF-16 で 2、UTF-8 で 4 バイト
        assert_eq!(utf16_to_byte_in("𝔸x", 2), 4);
    }
}
