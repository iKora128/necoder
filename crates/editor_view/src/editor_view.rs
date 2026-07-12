//! editor_view — [`editor_core::Buffer`] を GPUI で描く編集ビュー。
//!
//! 参考: zed `crates/gpui/examples/input.rs`（単一行）と `crates/editor`（本物）。ここは M2 用の
//! 素朴な複数行版: 行仮想化・行番号ガター・キャレット・縦スクロール・キーボード/マウス・IME。
//! 色は theme_core、文字列は i18n（`t!`）経由（UI-SPEC の許可リスト厳守）。

use editor_core::{Buffer, BufferSnapshot, Point as BufferPoint, Selection};
use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, InspectorElementId,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString, Style, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, actions, div, fill, hsla, point, prelude::*, px,
    relative, size,
};
use std::ops::Range;
use theme_core::{SyntaxColors, Theme};

const FONT_SIZE: f32 = 13.0; // code 13px
const LINE_HEIGHT: f32 = 23.0; // compact 行高 23
const GUTTER_PADDING: f32 = 10.0; // 行番号の左右余白
const GUTTER_MIN_WIDTH: f32 = 46.0; // UI-SPEC §1.4 行番号ガター 46

actions!(
    editor,
    [
        Backspace,
        Delete,
        Newline,
        InsertNewline,
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
    // キャレット点滅。focus 中のみ動かし、blur で止める（idle CPU 0% を守る）。
    blink_visible: bool,
    _blink_task: Option<gpui::Task<()>>,
    // 構文ハイライト（対応拡張子のみ）。編集で再計算する。
    highlighter: Option<lang::Highlighter>,
    highlights: Vec<lang::HighlightSpan>,
}

/// バッファをハイライトする。対応言語かつ一定サイズ以下のときだけ（巨大ファイルの毎編集再解析を避ける）。
fn compute_highlights(
    highlighter: &Option<lang::Highlighter>,
    buffer: &Buffer,
) -> Vec<lang::HighlightSpan> {
    const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
    match highlighter {
        Some(highlighter) if buffer.len_bytes() <= MAX_HIGHLIGHT_BYTES => {
            highlighter.highlight(&buffer.text())
        }
        _ => Vec::new(),
    }
}

impl EditorView {
    pub fn new(buffer: Buffer, theme: Theme, accent: gpui::Hsla, cx: &mut Context<Self>) -> Self {
        let highlighter = buffer
            .path()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .and_then(lang::Highlighter::for_extension);
        let highlights = compute_highlights(&highlighter, &buffer);
        Self {
            buffer,
            focus_handle: cx.focus_handle(),
            theme,
            accent,
            plain: false,
            submit_on_enter: false,
            highlighter,
            highlights,
            scroll_top: px(0.),
            marked_range: None,
            content_origin: None,
            viewport_height: px(0.),
            gutter_width: px(0.),
            caret_bounds: None,
            is_selecting: false,
            blink_visible: true,
            _blink_task: None,
        }
    }

    /// キャレット点滅を開始する（focus 時に呼ぶ）。530ms ごとに反転して notify。
    /// `_blink_task` を差し替える＝古いタスクは drop されて止まる。blur 時は `_blink_task=None`。
    fn start_blinking(&mut self, cx: &mut Context<Self>) {
        self.blink_visible = true;
        self._blink_task = Some(cx.spawn(async move |editor, cx| {
            loop {
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

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// 現在のテキスト全体（composer が送信時に読む）。
    pub fn plain_text(&self) -> String {
        self.buffer.text()
    }

    /// テキストを空に戻す（composer 送信後）。
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.buffer = Buffer::new();
        self.marked_range = None;
        self.highlights = Vec::new();
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
        let extension = self.buffer.path()?.extension()?.to_str()?;
        let name = match extension {
            "rs" => "Rust",
            "toml" => "TOML",
            "md" | "markdown" => "Markdown",
            "json" => "JSON",
            "js" | "mjs" | "cjs" => "JavaScript",
            "ts" | "tsx" => "TypeScript",
            "py" => "Python",
            "html" | "htm" => "HTML",
            "css" => "CSS",
            "yml" | "yaml" => "YAML",
            "sh" | "zsh" | "bash" => "Shell",
            "c" | "h" => "C",
            "cpp" | "cc" | "cxx" | "hpp" => "C++",
            "go" => "Go",
            "txt" => "Text",
            other => return Some(SharedString::from(other.to_uppercase())),
        };
        Some(SharedString::from(name))
    }

    /// キャレット・現在行アクセント色を設定する（アクティブプロジェクト色を流す）。
    pub fn set_accent(&mut self, color: gpui::Hsla, cx: &mut Context<Self>) {
        self.accent = color;
        cx.notify();
    }

    fn line_height(&self) -> Pixels {
        px(LINE_HEIGHT)
    }

    fn primary(&self) -> Selection {
        self.buffer.selections().first().copied().unwrap_or(Selection::cursor(0))
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
        self.scroll_caret_into_view();
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
        self.apply_cursor(false, |snapshot, selection| vertical(snapshot, selection.head, -1), cx);
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(false, |snapshot, selection| vertical(snapshot, selection.head, 1), cx);
    }

    fn move_to_line_start(&mut self, _: &MoveToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(false, |snapshot, selection| line_edge(snapshot, selection.head, false), cx);
    }

    fn move_to_line_end(&mut self, _: &MoveToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(false, |snapshot, selection| line_edge(snapshot, selection.head, true), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(true, |snapshot, selection| snapshot.prev_char_boundary(selection.head), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(true, |snapshot, selection| snapshot.next_char_boundary(selection.head), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(true, |snapshot, selection| vertical(snapshot, selection.head, -1), cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_cursor(true, |snapshot, selection| vertical(snapshot, selection.head, 1), cx);
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

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        // Enter 送信が有効なら、送信を親へ委ねる。ただし **IME 変換中（marked_range あり）は送信しない**
        // ＝日本語の変換確定 Enter で誤送信しないための肝（`docs/JOURNAL.md` の痛点）。
        if self.submit_on_enter && self.marked_range.is_none() {
            cx.emit(ComposerEvent::Submit);
            return;
        }
        self.buffer.insert("\n");
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

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.buffer.save() {
            eprintln!("保存に失敗: {error:#}");
        }
        cx.notify();
    }

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.marked_range = None;
        self.blink_visible = true; // 編集直後はキャレットを実体化（点滅 OFF で消えないよう）
        self.refresh_highlights();
        self.scroll_caret_into_view();
        cx.notify();
    }

    fn refresh_highlights(&mut self) {
        self.highlights = compute_highlights(&self.highlighter, &self.buffer);
    }

    // ── スクロール ──

    fn on_scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta_y = event.delta.pixel_delta(self.line_height()).y;
        self.scroll_top = (self.scroll_top - delta_y).max(px(0.)).min(self.max_scroll_top());
        cx.notify();
    }

    fn max_scroll_top(&self) -> Pixels {
        let total = self.line_height() * self.buffer.snapshot().line_count() as f32;
        (total - self.viewport_height).max(px(0.))
    }

    fn scroll_caret_into_view(&mut self) {
        if self.viewport_height <= px(0.) {
            return;
        }
        let row = self.buffer.snapshot().byte_to_point(self.primary().head).row;
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

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        self.blink_visible = true; // クリックでキャレットを置いた直後は実体化
        let offset = self.offset_for_position(event.position, window);
        if event.modifiers.shift {
            let anchor = self.primary().anchor;
            self.buffer.set_selections(vec![Selection::new(anchor, offset)]);
        } else {
            self.buffer.set_selections(vec![Selection::cursor(offset)]);
        }
        self.marked_range = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_selecting {
            return;
        }
        let offset = self.offset_for_position(event.position, window);
        let anchor = self.primary().anchor;
        self.buffer.set_selections(vec![Selection::new(anchor, offset)]);
        cx.notify();
    }

    fn offset_for_position(&self, position: Point<Pixels>, window: &mut Window) -> usize {
        let Some(origin) = self.content_origin else {
            return self.primary().head;
        };
        let snapshot = self.buffer.snapshot();
        let relative_y = (position.y - origin.y + self.scroll_top).max(px(0.));
        let row = ((f32::from(relative_y) / LINE_HEIGHT).floor() as usize)
            .min(snapshot.line_count().saturating_sub(1));
        let line_text = snapshot.line_text(row);
        let shaped = shape_plain(&line_text, self.theme.fg0, window);
        let local_x = (position.x - origin.x).max(px(0.));
        let column = shaped.closest_index_for_x(local_x);
        snapshot.point_to_byte(BufferPoint::new(row, column))
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

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Editor")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .when(!self.plain, |element| element.bg(self.theme.bg1))
            .text_color(self.theme.fg0)
            // コード = Guguru Sans Code（等幅・bin で bundle 済み）/ composer(plain) = IBM Plex Sans JP（UI）
            .font_family(if self.plain { "IBM Plex Sans JP" } else { "Guguru Sans Code" })
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
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
            .child(EditorElement { editor: cx.entity() })
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
            range: self.buffer.byte_to_utf16(selection.start())..self.buffer.byte_to_utf16(selection.end()),
            reversed: selection.head < selection.anchor,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
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
        let range = self.resolve_range(range_utf16);
        self.buffer.edit(&[range], new_text);
        self.marked_range = None;
        self.refresh_highlights();
        self.scroll_caret_into_view();
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
        self.scroll_caret_into_view();
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
        let view = self.editor.read(cx);
        let snapshot = view.buffer.snapshot();
        let theme = view.theme.clone();
        let accent = view.accent;
        let scroll_top = view.scroll_top;
        let marked = view.marked_range.clone();
        let focused = view.focus_handle.is_focused(window);
        let selections = view.buffer.selections().to_vec();
        let highlights = view.highlights.clone();
        let plain = view.plain;
        let blink_visible = view.blink_visible;
        let text_font = window.text_style().font();
        let primary = selections.first().copied().unwrap_or(Selection::cursor(0));

        let line_count = snapshot.line_count();
        let line_height = px(LINE_HEIGHT);

        // ガター幅: 最大行番号を shape して測る（平坦モードはガター無し）。
        let gutter_width = if plain {
            px(0.)
        } else {
            let widest_number = line_count.to_string();
            let sample = shape_plain(&widest_number, theme.fg2, window);
            px((f32::from(sample.width) + GUTTER_PADDING * 2.0).max(GUTTER_MIN_WIDTH))
        };

        let content_origin = point(bounds.left() + gutter_width, bounds.top());
        let content_width = (bounds.size.width - gutter_width).max(px(0.));
        let content_bounds = Bounds::new(content_origin, size(content_width, bounds.size.height));

        let visible = visible_rows(f32::from(scroll_top), f32::from(bounds.size.height), LINE_HEIGHT, line_count);

        let mut line_numbers = Vec::new();
        let mut text_lines = Vec::new();
        let mut selection_quads = Vec::new();
        let mut carets = Vec::new();
        let mut current_line = None;
        let mut caret_bounds = None;

        let primary_row = snapshot.byte_to_point(primary.head).row;

        for row in visible {
            let y = bounds.top() + line_height * (row as f32) - scroll_top;
            let line_text = snapshot.line_text(row);
            let line_start = snapshot.point_to_byte(BufferPoint::new(row, 0));
            let line_end = line_start + line_text.len();

            if !plain && row == primary_row {
                current_line = Some(fill(
                    Bounds::new(point(content_origin.x, y), size(content_width, line_height)),
                    hsla(0., 0., 1., 0.045),
                ));
            }

            // 行番号（右寄せ・平坦モードは無し）
            if !plain {
                let number_text = (row + 1).to_string();
                let number_line = shape_plain(&number_text, theme.fg2, window);
                let number_x = bounds.left() + gutter_width - px(GUTTER_PADDING) - number_line.width;
                line_numbers.push(PositionedLine { line: number_line, origin: point(number_x, y) });
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
                px(FONT_SIZE),
                &runs,
                None,
            );

            // 選択面
            for selection in &selections {
                if selection.is_empty() || selection.end() <= line_start || selection.start() > line_end {
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

            // キャレット（空選択）
            for (index, selection) in selections.iter().enumerate() {
                if !selection.is_empty() || snapshot.byte_to_point(selection.head).row != row {
                    continue;
                }
                let caret_x = content_origin.x + shaped.x_for_index(selection.head - line_start);
                let caret_rect = Bounds::new(point(caret_x, y), size(px(2.), line_height));
                carets.push(fill(caret_rect, accent));
                if index == 0 {
                    caret_bounds = Some(caret_rect);
                }
            }

            text_lines.push(PositionedLine { line: shaped, origin: point(content_origin.x, y) });
        }

        // focus 中かつ点滅 ON のフレームだけキャレットを出す。
        if !focused || !blink_visible {
            carets.clear();
        }

        EditorPrepaint {
            line_numbers,
            text_lines,
            selections: selection_quads,
            carets,
            current_line,
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

        if let Some(highlight) = prepaint.current_line.take() {
            window.paint_quad(highlight);
        }
        for quad in prepaint.selections.drain(..) {
            window.paint_quad(quad);
        }
        let line_height = px(LINE_HEIGHT);
        for positioned in prepaint.line_numbers.drain(..) {
            if let Err(error) = positioned.line.paint(positioned.origin, line_height, TextAlign::Left, None, window, cx) {
                eprintln!("gutter paint 失敗: {error}");
            }
        }
        for positioned in prepaint.text_lines.drain(..) {
            if let Err(error) = positioned.line.paint(positioned.origin, line_height, TextAlign::Left, None, window, cx) {
                eprintln!("text paint 失敗: {error}");
            }
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
            // 点滅の start/stop を focus に連動させる（blur 中は止めて idle 0% を守る）。
            if focused {
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
fn visible_rows(scroll_top: f32, viewport_height: f32, line_height: f32, line_count: usize) -> Range<usize> {
    if line_height <= 0.0 {
        return 0..line_count;
    }
    let first = (scroll_top / line_height).floor().max(0.0) as usize;
    let last = ((scroll_top + viewport_height) / line_height).ceil() as usize;
    first.min(line_count)..last.min(line_count)
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
        let color = line_spans
            .iter()
            .find(|span| span.range.start <= absolute && absolute < span.range.end)
            .map(|span| syntax_color(span.kind, syntax))
            .unwrap_or(default_color);
        let underlined = marked
            .as_ref()
            .is_some_and(|range| absolute >= range.start && absolute < range.end);
        let char_len = character.len_utf8();
        match runs.last_mut() {
            Some(run) if run.color == color && run.underline.is_some() == underlined => {
                run.len += char_len;
            }
            _ => runs.push(TextRun {
                len: char_len,
                font: text_font.clone(),
                color,
                background_color: None,
                underline: underlined.then(|| UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.0),
                    wavy: false,
                }),
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
    }
}

/// 単色 1 行を shape する（行番号・ヒットテスト用）。フォントはウィンドウの解決済みフォントを使う。
fn shape_plain(text: &str, color: gpui::Hsla, window: &mut Window) -> ShapedLine {
    let text_font = window.text_style().font();
    let run = TextRun {
        len: text.len(),
        font: text_font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(SharedString::from(text.to_string()), px(FONT_SIZE), &[run], None)
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
    fn utf16_to_byte_in_handles_japanese_and_astral() {
        assert_eq!(utf16_to_byte_in("あい", 0), 0);
        assert_eq!(utf16_to_byte_in("あい", 1), 3); // "あ" = 3 バイト
        assert_eq!(utf16_to_byte_in("あい", 2), 6);
        // "𝔸" は UTF-16 で 2、UTF-8 で 4 バイト
        assert_eq!(utf16_to_byte_in("𝔸x", 2), 4);
    }
}
