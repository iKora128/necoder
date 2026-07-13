//! terminal_view — 統合ターミナル（M8）。`alacritty_terminal` を GPUI に載せる最小実装。
//!
//! 移植根拠は `docs/research/porting-git-terminal-lsp.md`。設計の要点:
//! - **EventLoop が読取スレッド + vte parser**（自前で書かない）。PTY 出力 → parse → `Term` 更新 →
//!   `EventListener::send_event(Wakeup)`。
//! - **idle 0%**: 出力時のみ Wakeup → pump（`cx.spawn`）→ `sync`（スナップショット + notify）。**タイマー無し**。
//! - 入力は v1 では `on_key_down` 一本化（印字も特殊キーも bytes 化）。IME 前編集は非対応（後続）。
//! - 出典: zed `terminal` / `terminal_view`（GPL-3.0-or-later の設計を参考に新規実装。2026-07 時点）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as AlacEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point as AlacPoint;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};

use gpui::{
    App, Bounds, Context, Element, ElementId, Entity, FocusHandle, Focusable, GlobalElementId,
    Hsla, InspectorElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton, Pixels, Rgba,
    SharedString, Style, TextRun, Window, div, fill, point, prelude::*, px, size,
};
use theme_core::Theme;

/// ターミナルのセル寸法。フォントメトリクスと矛盾しないよう prepaint で決める。
const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT: f32 = 17.0;

/// 行列サイズ（alacritty の [`Dimensions`] を満たす）。
#[derive(Clone, Copy, PartialEq, Eq)]
struct TerminalSize {
    columns: usize,
    lines: usize,
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// alacritty の背景スレッド（EventLoop）から GPUI 前景へイベントを渡す橋渡し。
#[derive(Clone)]
struct Listener(UnboundedSender<AlacEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: AlacEvent) {
        // 受信側（TerminalView）が drop 済み＝チャネル閉＝正常終了。ここでの失敗は無害。
        let _ = self.0.unbounded_send(event);
    }
}

/// 描画用に切り出した 1 セル（すべて Copy。ロックを短く保つため所有コピーする）。
#[derive(Clone, Copy)]
struct RenderCell {
    point: AlacPoint,
    character: char,
    fg: AnsiColor,
    bg: AnsiColor,
    flags: Flags,
}

/// term をロックして取り出した表示スナップショット（前景でロック無しに読める）。
#[derive(Default)]
struct TerminalContent {
    cells: Vec<RenderCell>,
    cursor: Option<AlacPoint>,
}

/// 統合ターミナル（モデル + ビューを兼ねる 1 エンティティ）。
pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    /// PTY への入力/リサイズ送信。生成失敗時は `None`（死んだ端末）。
    notifier: Option<Notifier>,
    content: TerminalContent,
    size: TerminalSize,
    /// アプリケーションカーソルモード（矢印キーのエスケープが変わる）。
    app_cursor: bool,
    exited: bool,
    theme: Theme,
    focus_handle: FocusHandle,
    // pump タスク（PTY 出力で起きる）。drop で停止。IO スレッド自体は spawn 後 detach し、
    // Drop の Msg::Shutdown で畳む。
    _pump: Option<gpui::Task<()>>,
}

impl TerminalView {
    /// `cwd` でシェルを起動する。生成に失敗しても「死んだ端末」を返す（クラッシュさせない）。
    pub fn new(cwd: Option<PathBuf>, theme: Theme, cx: &mut Context<Self>) -> Self {
        Self::new_with_shell(cwd, None, theme, cx)
    }

    /// shell command を明示して PTY を起動する。Remote SSH はここへ `ssh -tt ...` を渡す。
    pub fn new_with_shell(
        cwd: Option<PathBuf>,
        shell: Option<(String, Vec<String>)>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Self {
        let (events_tx, mut events_rx) = unbounded::<AlacEvent>();
        let listener = Listener(events_tx);
        let size = TerminalSize { columns: 80, lines: 24 };
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));

        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        let options = tty::Options {
            shell: shell.map(|(program, args)| tty::Shell::new(program, args)),
            working_directory: cwd,
            drain_on_exit: true,
            env,
            ..Default::default()
        };
        let window_size = WindowSize {
            num_lines: size.lines as u16,
            num_cols: size.columns as u16,
            cell_width: 8,
            cell_height: 16,
        };

        let mut notifier = None;
        let mut pump = None;
        match tty::new(&options, window_size, 0) {
            Ok(pty) => match EventLoop::new(term.clone(), listener, pty, true, false) {
                Ok(event_loop) => {
                    notifier = Some(Notifier(event_loop.channel()));
                    // IO スレッドを起動して detach（JoinHandle は保持しない。Drop の Shutdown で畳む）。
                    event_loop.spawn();
                    // pump: PTY 出力（Wakeup 等）でのみ起きる＝idle 0%。
                    pump = Some(cx.spawn(async move |view, cx| {
                        while let Some(event) = events_rx.next().await {
                            let closed = view
                                .update(cx, |view, cx| view.on_alac_event(event, cx))
                                .is_err();
                            if closed {
                                break;
                            }
                        }
                    }));
                }
                Err(error) => eprintln!("ターミナル: EventLoop 生成失敗: {error}"),
            },
            Err(error) => eprintln!("ターミナル: PTY 生成失敗: {error}"),
        }

        let exited = notifier.is_none();
        Self {
            term,
            notifier,
            content: TerminalContent::default(),
            size,
            app_cursor: false,
            exited,
            theme,
            focus_handle: cx.focus_handle(),
            _pump: pump,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// テーマ差し替え（テーマセレクタ連動）。
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// alacritty イベントを処理する（前景・pump から）。Wakeup で再スナップショット。
    fn on_alac_event(&mut self, event: AlacEvent, cx: &mut Context<Self>) {
        match event {
            AlacEvent::Wakeup => self.sync(cx),
            AlacEvent::Exit => {
                self.exited = true;
                cx.notify();
            }
            // アプリがPTYへ書き戻しを要求（端末問い合わせ応答等）。
            AlacEvent::PtyWrite(text) => self.write_bytes(text.into_bytes()),
            _ => {}
        }
    }

    /// term をロックして表示スナップショットを取り直し、再描画を促す。
    fn sync(&mut self, cx: &mut Context<Self>) {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cells = content
            .display_iter
            .map(|indexed| RenderCell {
                point: indexed.point,
                character: indexed.cell.c,
                fg: indexed.cell.fg,
                bg: indexed.cell.bg,
                flags: indexed.cell.flags,
            })
            .collect();
        let cursor = Some(content.cursor.point);
        self.app_cursor = term.mode().contains(TermMode::APP_CURSOR);
        drop(term);
        self.content = TerminalContent { cells, cursor };
        cx.notify();
    }

    /// PTY へ入力バイトを送る。
    fn write_bytes(&self, bytes: Vec<u8>) {
        if let Some(notifier) = &self.notifier {
            notifier.notify(bytes);
        }
    }

    /// 行列サイズが変わったら term と PTY をリサイズする（prepaint から）。
    fn resize(&mut self, columns: usize, lines: usize) {
        let new_size = TerminalSize { columns: columns.max(2), lines: lines.max(1) };
        if new_size == self.size {
            return;
        }
        self.size = new_size;
        self.term.lock().resize(new_size);
        if let Some(notifier) = &self.notifier {
            let window_size = WindowSize {
                num_lines: new_size.lines as u16,
                num_cols: new_size.columns as u16,
                cell_width: 8,
                cell_height: 16,
            };
            if let Err(error) = notifier.0.send(Msg::Resize(window_size)) {
                eprintln!("ターミナル: リサイズ送信に失敗: {error}");
            }
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bytes) = keystroke_to_bytes(&event.keystroke, self.app_cursor) {
            self.write_bytes(bytes);
            cx.stop_propagation();
        }
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        // EventLoop（IO スレッド）を止める。チャネル閉なら既に終了。
        if let Some(notifier) = &self.notifier {
            let _ = notifier.0.send(Msg::Shutdown);
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            // クリックでフォーカス（キー入力を受けるように）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                }),
            )
            .size_full()
            .bg(self.theme.bg1)
            .child(TerminalElement { terminal: cx.entity() })
    }
}

// ── キー → PTY バイト（v1 の最小マッピング） ──

/// キーストロークを PTY へ送るバイト列へ。特殊キー + 印字（key_char）を扱う。対象外は `None`。
fn keystroke_to_bytes(keystroke: &gpui::Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let modifiers = keystroke.modifiers;
    let key = keystroke.key.as_str();

    // Ctrl + 英字 → 制御バイト（Ctrl-A=0x01 .. Ctrl-Z=0x1a）。Ctrl-C 等。
    if modifiers.control && !modifiers.platform {
        if key.len() == 1 {
            let character = key.as_bytes()[0];
            if character.is_ascii_alphabetic() {
                return Some(vec![(character.to_ascii_lowercase() - b'a') + 1]);
            }
        }
    }
    // ⌘ 系はターミナルに送らない（コピー等は今は素通り）。
    if modifiers.platform {
        return None;
    }

    let bytes: &[u8] = match key {
        "enter" => b"\r",
        "backspace" => b"\x7f",
        "tab" => b"\t",
        "escape" => b"\x1b",
        "up" => {
            if app_cursor {
                b"\x1bOA"
            } else {
                b"\x1b[A"
            }
        }
        "down" => {
            if app_cursor {
                b"\x1bOB"
            } else {
                b"\x1b[B"
            }
        }
        "right" => {
            if app_cursor {
                b"\x1bOC"
            } else {
                b"\x1b[C"
            }
        }
        "left" => {
            if app_cursor {
                b"\x1bOD"
            } else {
                b"\x1b[D"
            }
        }
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "delete" => b"\x1b[3~",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        _ => {
            // 印字文字（key_char に確定した文字が入る）。
            if let Some(text) = &keystroke.key_char {
                if !text.is_empty() && !text.chars().any(char::is_control) {
                    return Some(text.as_bytes().to_vec());
                }
            }
            return None;
        }
    };
    Some(bytes.to_vec())
}

// ── ANSI 色 → Hsla（テーマの fg/bg を既定色に流用・16/256 色は標準パレット） ──

/// VSCode 系の 16 色 ANSI パレット（normal 8 + bright 8）。
const ANSI16: [(u8, u8, u8); 16] = [
    (30, 30, 30),
    (205, 49, 49),
    (13, 188, 121),
    (229, 229, 16),
    (36, 114, 200),
    (188, 63, 188),
    (17, 168, 205),
    (229, 229, 229),
    (102, 102, 102),
    (241, 76, 76),
    (35, 209, 139),
    (245, 245, 67),
    (59, 142, 234),
    (214, 112, 214),
    (41, 184, 219),
    (255, 255, 255),
];

fn rgb_hsla(red: u8, green: u8, blue: u8) -> Hsla {
    Rgba {
        r: red as f32 / 255.0,
        g: green as f32 / 255.0,
        b: blue as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

fn named_to_hsla(named: NamedColor, theme: &Theme) -> Hsla {
    let index = match named {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        NamedColor::Foreground => return theme.fg0,
        NamedColor::Background => return theme.bg1,
        NamedColor::Cursor => return theme.fg0,
        _ => return theme.fg0,
    };
    let (red, green, blue) = ANSI16[index];
    rgb_hsla(red, green, blue)
}

fn indexed_to_hsla(index: u8) -> Hsla {
    match index {
        0..=15 => {
            let (red, green, blue) = ANSI16[index as usize];
            rgb_hsla(red, green, blue)
        }
        16..=231 => {
            // 6×6×6 カラーキューブ（各成分 0 or c*40+55）。
            let value = index - 16;
            let convert = |component: u8| -> u8 {
                if component == 0 {
                    0
                } else {
                    component * 40 + 55
                }
            };
            rgb_hsla(convert(value / 36), convert((value / 6) % 6), convert(value % 6))
        }
        _ => {
            // グレースケール（232..255）。
            let level = (index - 232) * 10 + 8;
            rgb_hsla(level, level, level)
        }
    }
}

fn ansi_to_hsla(color: AnsiColor, theme: &Theme) -> Hsla {
    match color {
        AnsiColor::Named(named) => named_to_hsla(named, theme),
        AnsiColor::Spec(rgb) => rgb_hsla(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(index) => indexed_to_hsla(index),
    }
}

fn is_default_background(color: AnsiColor) -> bool {
    matches!(color, AnsiColor::Named(NamedColor::Background))
}

// ── 描画（custom Element でグリッドを塗る） ──

struct TerminalElement {
    terminal: Entity<TerminalView>,
}

struct TerminalPrepaint {
    cells: Vec<RenderCell>,
    cursor: Option<AlacPoint>,
    cell_width: Pixels,
    line_height: Pixels,
    origin: gpui::Point<Pixels>,
    focused: bool,
    theme: Theme,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
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
        style.size.width = gpui::relative(1.0).into();
        style.size.height = gpui::relative(1.0).into();
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
        // 等幅セル幅を 'M' を shape して測る。
        let font = window.text_style().font();
        let sample = window.text_system().shape_line(
            SharedString::from("M"),
            px(FONT_SIZE),
            &[TextRun {
                len: 1,
                font: font.clone(),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let cell_width = if sample.width > px(0.) {
            sample.width
        } else {
            px(FONT_SIZE * 0.6)
        };
        let line_height = px(LINE_HEIGHT);

        // 行列サイズを算出して term をリサイズ。
        let columns = (f32::from(bounds.size.width) / f32::from(cell_width)).floor().max(2.0) as usize;
        let lines = (f32::from(bounds.size.height) / f32::from(line_height)).floor().max(1.0) as usize;
        let theme = self.terminal.read(cx).theme.clone();
        let (cells, cursor, focused) = {
            let focused = self.terminal.read(cx).focus_handle.is_focused(window);
            self.terminal.update(cx, |terminal, _cx| {
                terminal.resize(columns, lines);
                (terminal.content.cells.clone(), terminal.content.cursor, focused)
            })
        };

        TerminalPrepaint {
            cells,
            cursor,
            cell_width,
            line_height,
            origin: bounds.origin,
            focused,
            theme,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = &prepaint.theme;
        let origin = prepaint.origin;
        let cell_width = prepaint.cell_width;
        let line_height = prepaint.line_height;
        let font = window.text_style().font();

        // 面全体の背景。
        window.paint_quad(fill(bounds, theme.bg1));

        // ① 既定でない背景セルの矩形。
        for cell in &prepaint.cells {
            let (mut foreground, mut background) = (cell.fg, cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            if !is_default_background(background) {
                let position = point(
                    origin.x + cell_width * (cell.point.column.0 as f32),
                    origin.y + line_height * (cell.point.line.0 as f32),
                );
                window.paint_quad(fill(
                    Bounds::new(position, size(cell_width, line_height)),
                    ansi_to_hsla(background, theme),
                ));
            }
        }

        // ② カーソル（focus 時は塗りブロック・非focus は輪郭）。
        if let Some(cursor) = prepaint.cursor {
            let position = point(
                origin.x + cell_width * (cursor.column.0 as f32),
                origin.y + line_height * (cursor.line.0 as f32),
            );
            let cursor_bounds = Bounds::new(position, size(cell_width, line_height));
            if prepaint.focused {
                window.paint_quad(fill(cursor_bounds, theme.fg1));
            } else {
                window.paint_quad(gpui::outline(
                    cursor_bounds,
                    theme.fg2,
                    gpui::BorderStyle::default(),
                ));
            }
        }

        // ③ セル文字（1 セル 1 shape。v1 はバッチ無し）。
        for cell in &prepaint.cells {
            if cell.character == ' ' || cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::HIDDEN) {
                continue;
            }
            let (mut foreground, mut background) = (cell.fg, cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            // カーソル下の文字は視認性のため背景色で描く。
            let on_cursor = prepaint.focused
                && prepaint.cursor.is_some_and(|cursor| cursor == cell.point);
            let color = if on_cursor {
                theme.bg1
            } else {
                ansi_to_hsla(foreground, theme)
            };
            let mut cell_font = font.clone();
            if cell.flags.contains(Flags::BOLD) {
                cell_font.weight = gpui::FontWeight::BOLD;
            }
            if cell.flags.contains(Flags::ITALIC) {
                cell_font.style = gpui::FontStyle::Italic;
            }
            let run = TextRun {
                len: cell.character.len_utf8(),
                font: cell_font,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let position = point(
                origin.x + cell_width * (cell.point.column.0 as f32),
                origin.y + line_height * (cell.point.line.0 as f32),
            );
            let shaped = window.text_system().shape_line(
                SharedString::from(cell.character.to_string()),
                px(FONT_SIZE),
                &[run],
                Some(cell_width),
            );
            if let Err(error) =
                shaped.paint(position, line_height, gpui::TextAlign::Left, None, window, cx)
            {
                eprintln!("ターミナル文字描画に失敗: {error}");
            }
        }
    }
}
