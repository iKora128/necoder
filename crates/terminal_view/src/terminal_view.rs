//! terminal_view — 統合ターミナル（M8）。`alacritty_terminal` を GPUI に載せる最小実装。
//!
//! 独立実装の設計根拠は `docs/research/git-terminal-lsp-design-notes.md`。設計の要点:
//! - **EventLoop が読取スレッド + vte parser**（自前で書かない）。PTY 出力 → parse → `Term` 更新 →
//!   `EventListener::send_event(Wakeup)`。
//! - **idle 0%**: 出力時のみ Wakeup → pump（`cx.spawn`）→ `sync`（スナップショット + notify）。**タイマー無し**。
//! - 通常キーは `on_key_down`、IME は `EntityInputHandler`。前編集は保持し、確定文字だけ PTY へ送る。
//! - 公開 `alacritty_terminal` API と GPUI API を使った necoder 固有の実装。Zed の terminal code は取り込まない。

mod dock;
pub use dock::{TerminalDock, TerminalDockEvent, TerminalLaunch};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as AlacEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as AlacPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};

use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;

use gpui::{
    div, fill, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context, CursorStyle,
    DispatchPhase, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Rgba, ScrollWheelEvent, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use std::ops::Range;
use theme_core::Theme;

/// ターミナル → ホスト（workspace）への通知（M13: file:line リンク）。
pub enum TerminalEvent {
    /// `path:line` リンクのクリック。パスは端末出力のまま（相対は cwd 基準で解決してもらう）。
    OpenPath { path: String, line: u32 },
}

/// 表示セルから行を再構成してリンクを検出する。戻りは (表示行, セル列範囲, パス, 行番号)。
fn detect_links(cells: &[RenderCell]) -> Vec<(i32, Range<usize>, String, u32)> {
    let mut rows: std::collections::BTreeMap<i32, Vec<(usize, char)>> =
        std::collections::BTreeMap::new();
    for cell in cells {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        rows.entry(cell.point.line.0)
            .or_default()
            .push((cell.point.column.0 as usize, cell.character));
    }
    let mut links = Vec::new();
    for (line, row) in rows {
        let text: String = row.iter().map(|(_, character)| *character).collect();
        // 検出は `ui::links`（agent_panel の transcript と共有）。ターミナルは cargo/grep の
        // 出力が相手なので**行番号つきだけ**をリンクにする（実在確認をしないぶん厳しくする）。
        let offsets: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
        for link in ui::links::find_links(&text) {
            let ui::links::LinkTarget::Path {
                path,
                line: Some(line_number),
                ..
            } = link.target
            else {
                continue;
            };
            // byte 範囲 → セル列（全角が混ざる行でもずれないよう char 添字を経由する）。
            let Ok(first) = offsets.binary_search(&link.range.start) else {
                continue;
            };
            let last = match offsets.binary_search(&link.range.end) {
                Ok(index) => index,
                Err(index) => index,
            };
            let (Some((start_column, _)), Some((end_column, _))) =
                (row.get(first), row.get(last.saturating_sub(1)))
            else {
                continue;
            };
            links.push((line, *start_column..*end_column + 1, path, line_number));
        }
    }
    links
}

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

/// 開発用の計測口: `NECODER_TERM_PROBE=1` で PTY → pump → sync → 描画の到達を stderr に出す。
///
/// 「端末のドックは開くのに中身が真っ黒」のとき、**どこで止まっているか**は外から見えない
/// （2026-08-23・Windows で実際に詰まった）。PTY 自体は `tests/pty_smoke.rs` が保証するので、
/// ここは**その先の経路**専用。既定 off ＝ 通常の実行には一切影響しない。
fn term_probe(stage: &str, detail: impl std::fmt::Display) {
    if std::env::var_os("NECODER_TERM_PROBE").is_some() {
        eprintln!("[term-probe] {stage}: {detail}");
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
    /// マウス選択の範囲（グリッド座標）。ハイライト描画と ⌘C コピーに使う。
    selection: Option<SelectionRange>,
    /// スクロールバックの表示オフセット（0 = 最下段）。セルはグリッド座標のまま持つので、
    /// 描画時に `line + display_offset` で表示行へ写す。
    display_offset: usize,
}

/// ドラッグ選択の最新スナップショット（ポインタ位置 + そのフレームの描画座標）。
#[derive(Clone, Copy)]
struct DragFrame {
    position: gpui::Point<Pixels>,
    origin: gpui::Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
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
    /// OS に変換中の範囲を返すための前編集。PTY には確定するまで送らない。
    marked_text: String,
    marked_selection: Range<usize>,
    #[cfg(test)]
    written_input: std::cell::RefCell<Vec<u8>>,
    /// マウス左ボタンを押してドラッグ選択中か（move で範囲を延ばす判定）。
    selecting: bool,
    /// ドラッグ選択中の最新ポインタ位置とフレーム座標。ビュー外へ引っ張った時の
    /// 自動スクロール tick が選択の引き直しに使う（up で消える）。
    drag_frame: Option<DragFrame>,
    /// 自動スクロールの tick ループが走っているか（二重 spawn 防止）。
    drag_autoscroll_running: bool,
    /// ホイールの 1 行未満の端数持ち越し（トラックパッドのピクセル増分を行単位に畳む）。
    scroll_remainder: f32,
    exited: bool,
    /// 前面でシェル以外のプロセスが動いているかを調べる口（閉じる前の確認・O4）。
    #[cfg(unix)]
    foreground: Option<ForegroundProbe>,
    theme: Theme,
    focus_handle: FocusHandle,
    // pump タスク（PTY 出力で起きる）。drop で停止。IO スレッド自体は spawn 後 detach し、
    // Drop の Msg::Shutdown で畳む。
    _pump: Option<gpui::Task<()>>,
}

/// 端末の前面でシェル以外のプロセス（`npm run dev`・`vim`・`claude` 等）が動いているかを調べる口。
///
/// PTY 本体は EventLoop のスレッドへ渡してしまうので、生成の直後に **master の fd を複製**して
/// 手元に残す（`tcgetpgrp` で前面のプロセスグループを読むだけ＝入出力には触れない）。
/// シェルは起動時に `setsid` で自分のプロセスグループの長になっている（alacritty の起動手順）ので、
/// 前面のグループがシェル自身の pid と違えば「ジョブが前面で動いている」。
#[cfg(unix)]
struct ForegroundProbe {
    master: std::fs::File,
    shell_pid: libc::pid_t,
}

#[cfg(unix)]
impl ForegroundProbe {
    fn new(pty: &tty::Pty) -> Option<Self> {
        let shell_pid = libc::pid_t::try_from(pty.child().id()).ok()?;
        match pty.file().try_clone() {
            Ok(master) => Some(Self { master, shell_pid }),
            Err(error) => {
                eprintln!(
                    "ターミナル: 前面プロセスの確認口を作れない（閉じる時の確認を省く）: {error}"
                );
                None
            }
        }
    }

    fn has_foreground_job(&self) -> bool {
        use std::os::fd::AsRawFd as _;
        // SAFETY: `master` は生きている File の fd。`tcgetpgrp` は端末の前面グループを読むだけ。
        let group = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        foreground_is_busy(group, self.shell_pid)
    }
}

/// 前面のプロセスグループ `group` がシェル（`shell_pid`）以外なら「何かが前面で動いている」。
/// 読めなかった（負）・0・シェル自身は「動いていない」＝確認しない側に倒す。
#[cfg(unix)]
fn foreground_is_busy(group: libc::pid_t, shell_pid: libc::pid_t) -> bool {
    group > 0 && group != shell_pid
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
        let size = TerminalSize {
            columns: 80,
            lines: 24,
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));

        let mut env = HashMap::new();
        if shell.is_none() {
            if let Some(root) = &cwd {
                match host::task_environment(host::LocalHost::shared().as_ref(), root) {
                    Ok(task_env) => env.extend(task_env),
                    Err(error) => eprintln!("terminal task.env: {error:#}"),
                }
            }
        }
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
        let pty = tty::new(&options, window_size, 0);
        // PTY は下で EventLoop へ渡る。前面グループを読む口だけは先に手元へ残す。
        #[cfg(unix)]
        let foreground = pty.as_ref().ok().and_then(ForegroundProbe::new);
        match pty {
            Ok(pty) => match EventLoop::new(term.clone(), listener, pty, true, false) {
                Ok(event_loop) => {
                    notifier = Some(Notifier(event_loop.channel()));
                    // IO スレッドを起動して detach（JoinHandle は保持しない。Drop の Shutdown で畳む）。
                    event_loop.spawn();
                    term_probe("pty", "起動");
                    // pump: PTY 出力（Wakeup 等）でのみ起きる＝idle 0%。
                    pump = Some(cx.spawn(async move |view, cx| {
                        let mut received = 0_usize;
                        while let Some(event) = events_rx.next().await {
                            received += 1;
                            term_probe("pump", format_args!("{received} 件目 {event:?}"));
                            let closed = view
                                .update(cx, |view, cx| view.on_alac_event(event, cx))
                                .is_err();
                            if closed {
                                break;
                            }
                        }
                        term_probe("pump", format_args!("終了（計 {received} 件）"));
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
            marked_text: String::new(),
            marked_selection: 0..0,
            #[cfg(test)]
            written_input: std::cell::RefCell::new(Vec::new()),
            selecting: false,
            drag_frame: None,
            drag_autoscroll_running: false,
            scroll_remainder: 0.0,
            exited,
            #[cfg(unix)]
            foreground,
            theme,
            focus_handle: cx.focus_handle(),
            _pump: pump,
        }
    }

    /// PTY スレッドを起動せず、Dock のライフサイクルを決定論的に検証するための端末。
    ///
    /// `gpui::test` のスケジューラは外部スレッドからの Wakeup を禁止するため、
    /// test-support feature でだけ公開する。本番の生成経路には入らない。
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn new_test(theme: Theme, cx: &mut Context<Self>) -> Self {
        let (events_tx, _events_rx) = unbounded::<AlacEvent>();
        let listener = Listener(events_tx);
        let size = TerminalSize {
            columns: 80,
            lines: 24,
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener)));
        Self {
            term,
            notifier: None,
            content: TerminalContent::default(),
            size,
            app_cursor: false,
            marked_text: String::new(),
            marked_selection: 0..0,
            #[cfg(test)]
            written_input: std::cell::RefCell::new(Vec::new()),
            selecting: false,
            drag_frame: None,
            drag_autoscroll_running: false,
            scroll_remainder: 0.0,
            exited: false,
            #[cfg(unix)]
            foreground: None,
            theme,
            focus_handle: cx.focus_handle(),
            _pump: None,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// 前面でシェル以外のプロセスが動いているか（閉じる前の確認の判定・O4）。
    ///
    /// 調べられない端末は `false` ＝確認しない側に倒す: Windows の ConPTY（前面グループの概念が無い）、
    /// リモート（ローカルの子は `ssh -tt` で、前面は常に ssh 自身）、終了済み・生成に失敗した端末。
    pub fn has_foreground_process(&self) -> bool {
        if self.exited {
            return false;
        }
        #[cfg(unix)]
        {
            self.foreground
                .as_ref()
                .is_some_and(ForegroundProbe::has_foreground_job)
        }
        #[cfg(not(unix))]
        {
            false
        }
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
        let display_offset = content.display_offset;
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
        // 選択範囲もスナップショットに含める（前景でロック無しにハイライトを描くため）。
        let selection = term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&term));
        drop(term);
        self.content = TerminalContent {
            cells,
            cursor,
            selection,
            display_offset,
        };
        term_probe(
            "sync",
            format_args!(
                "{} セル（うち非空白 {}）",
                self.content.cells.len(),
                self.content
                    .cells
                    .iter()
                    .filter(|cell| !cell.character.is_whitespace())
                    .count()
            ),
        );
        cx.notify();
    }

    /// PTY へ入力バイトを送る。
    fn write_bytes(&self, bytes: Vec<u8>) {
        #[cfg(test)]
        self.written_input.borrow_mut().extend_from_slice(&bytes);
        if let Some(notifier) = &self.notifier {
            notifier.notify(bytes);
        }
    }

    /// 外部（⌘I コマンド生成・M12-8）からテキストをタイプ入力として PTY へ送る。
    /// 改行は落とす＝**実行はしない**（実行はユーザーの Enter に委ねる）。
    pub fn insert_text(&self, text: &str) {
        let sanitized: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        if !sanitized.is_empty() {
            self.write_bytes(sanitized.into_bytes());
        }
    }

    /// 行列サイズが変わったら term と PTY をリサイズする（prepaint から）。
    fn resize(&mut self, columns: usize, lines: usize) {
        let new_size = TerminalSize {
            columns: columns.max(2),
            lines: lines.max(1),
        };
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
        // 変換候補の操作は IME のもの。未処理キーが流れてきても PTY へ漏らさない。
        if !self.marked_text.is_empty() {
            cx.stop_propagation();
            return;
        }
        let keystroke = &event.keystroke;
        // ⌘C = 選択コピー / ⌘V = 貼り付け。macOS 系のみ（⌃C は SIGINT のまま）。
        if keystroke.modifiers.platform && !keystroke.modifiers.control {
            match keystroke.key.as_str() {
                "c" => {
                    self.copy_selection(cx);
                    cx.stop_propagation();
                    return;
                }
                "v" => {
                    self.paste_from_clipboard(cx);
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        if let Some(bytes) = keystroke_to_bytes(&event.keystroke, self.app_cursor) {
            // タイプしたら最下段へ復帰（スクロールバック閲覧中の入力は現在行に届く＝端末の常識）。
            self.scroll_to_bottom(cx);
            self.write_bytes(bytes);
            // 入力したら選択は解除（出力でスクロールしても古いハイライトを残さない）。
            self.clear_selection(cx);
            cx.stop_propagation();
        }
    }

    /// ホイール/トラックパッドでスクロールバックを閲覧する。通常画面は表示オフセットを動かし、
    /// 代替画面（less / vim 等・履歴なし）は ALTERNATE_SCROLL に従い矢印キー相当を送る。
    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta_y = f32::from(event.delta.pixel_delta(px(LINE_HEIGHT)).y);
        let (lines, remainder) = wheel_lines(self.scroll_remainder, delta_y, LINE_HEIGHT);
        self.scroll_remainder = remainder;
        if lines == 0 {
            return;
        }
        let mut term = self.term.lock();
        let mode = *term.mode();
        if mode.contains(TermMode::ALT_SCREEN) {
            drop(term);
            // 代替画面にスクロールバックは無い。ALTERNATE_SCROLL が立っていれば
            // 上=↑ / 下=↓ を行数ぶん送ってアプリ側（less/vim）にスクロールさせる。
            if mode.contains(TermMode::ALTERNATE_SCROLL) {
                let code: &[u8] = match (lines > 0, self.app_cursor) {
                    (true, false) => b"\x1b[A",
                    (true, true) => b"\x1bOA",
                    (false, false) => b"\x1b[B",
                    (false, true) => b"\x1bOB",
                };
                let bytes = code.repeat(lines.unsigned_abs() as usize);
                self.write_bytes(bytes);
            }
        } else {
            term.scroll_display(Scroll::Delta(lines));
            drop(term);
            self.sync(cx);
        }
    }

    /// スクロールバックを畳んで最下段（現在行）へ戻す。
    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        if self.content.display_offset == 0 {
            return;
        }
        self.term.lock().scroll_display(Scroll::Bottom);
        self.sync(cx);
    }

    /// 選択テキストをクリップボードへ（選択が空なら何もしない＝⌘C の空打ちは無害）。
    fn copy_selection(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.term.lock().selection_to_string() {
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    /// クリップボードを PTY へ貼り付ける。bracketed paste モードなら囲み列を付ける
    /// （zsh 等が貼り付けを 1 塊として扱えるように）。
    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.scroll_to_bottom(cx);
        if self.term.lock().mode().contains(TermMode::BRACKETED_PASTE) {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            self.write_bytes(bytes);
        } else {
            self.write_bytes(text.into_bytes());
        }
    }

    /// 選択を解除してハイライトを消す（入力時など）。
    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        if term.selection.take().is_some() {
            drop(term);
            self.content.selection = None;
            cx.notify();
        }
    }

    /// ドラッグ選択の開始（左ボタン押下時）。押した位置をアンカーにする。
    fn begin_selection(
        &mut self,
        position: gpui::Point<Pixels>,
        origin: gpui::Point<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        cx: &mut Context<Self>,
    ) {
        let (row, column, side) =
            viewport_cell(position, origin, cell_width, line_height, self.size);
        self.selecting = true;
        let mut term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let point = AlacPoint::new(Line(row - offset), Column(column));
        term.selection = Some(Selection::new(SelectionType::Simple, point, side));
        let range = term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&term));
        drop(term);
        self.content.selection = range;
        cx.notify();
    }

    /// ドラッグ選択の延長（左ボタン保持で move したとき）。
    fn update_selection(
        &mut self,
        position: gpui::Point<Pixels>,
        origin: gpui::Point<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        let frame = DragFrame {
            position,
            origin,
            cell_width,
            line_height,
        };
        self.drag_frame = Some(frame);
        let (row, column, side) =
            viewport_cell(position, origin, cell_width, line_height, self.size);
        let mut term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let point = AlacPoint::new(Line(row - offset), Column(column));
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side);
        }
        let range = term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&term));
        drop(term);
        self.content.selection = range;
        // ビュー外へ引っ張ったら、押している間スクロールし続ける（下=最新側 / 上=履歴側）。
        if drag_overshoot(&frame, self.size) != 0.0 {
            self.start_drag_autoscroll(cx);
        }
        cx.notify();
    }

    /// ドラッグ自動スクロールの tick ループを起動する（既に走っていれば何もしない）。
    /// 代替画面（less / vim 等）にスクロールバックは無いので対象外。
    fn start_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.drag_autoscroll_running {
            return;
        }
        if self.term.lock().mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        self.drag_autoscroll_running = true;
        cx.spawn(async move |view, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(33))
                .await;
            let keep_going = view
                .update(cx, |view, cx| view.drag_autoscroll_tick(cx))
                .unwrap_or(false);
            if !keep_going {
                break;
            }
        })
        .detach();
    }

    /// 自動スクロール 1 tick: はみ出し量に応じて表示を送り、選択端を新オフセットで引き直す。
    /// 続行するなら true（選択終了・ビュー内復帰で自然停止）。
    fn drag_autoscroll_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let overshoot = match self.drag_frame {
            Some(frame) if self.selecting => drag_overshoot(&frame, self.size),
            _ => 0.0,
        };
        if overshoot == 0.0 {
            self.drag_autoscroll_running = false;
            return false;
        }
        let Some(frame) = self.drag_frame else {
            self.drag_autoscroll_running = false;
            return false;
        };
        // 遠くへ引くほど速く（1〜5 行/tick）。上はみ出し（負）= 履歴へ = Delta 正。
        let magnitude =
            (1 + (overshoot.abs() / f32::from(frame.line_height).max(1.0)) as i32).min(5);
        let lines = if overshoot < 0.0 {
            magnitude
        } else {
            -magnitude
        };
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.sync(cx);
        // 新しい display_offset で選択端を引き直す（端の行に吸着し続ける）。
        self.update_selection(
            frame.position,
            frame.origin,
            frame.cell_width,
            frame.line_height,
            cx,
        );
        true
    }

    /// ドラッグ選択の確定（左ボタン離し）。ドラッグ無し＝空選択は解除する。
    fn end_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        self.drag_frame = None;
        let mut term = self.term.lock();
        let empty = term
            .selection
            .as_ref()
            .is_none_or(|selection| selection.is_empty());
        if empty {
            term.selection = None;
        }
        let range = term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&term));
        drop(term);
        self.content.selection = range;
        cx.notify();
    }
}

/// ポインタの上下はみ出し量（px）。負 = ビュー上端より上（履歴方向）、正 = 下端より下。
/// ビュー内なら 0（自動スクロール停止の判定を兼ねる）。
fn drag_overshoot(frame: &DragFrame, size: TerminalSize) -> f32 {
    let top = f32::from(frame.origin.y);
    let bottom = top + size.lines as f32 * f32::from(frame.line_height);
    let y = f32::from(frame.position.y);
    if y < top {
        y - top
    } else if y > bottom {
        y - bottom
    } else {
        0.0
    }
}

/// ピクセル位置 → (グリッド行 i32, 列 usize, セル内の左右)。表示座標（display_offset 未適用）。
fn viewport_cell(
    position: gpui::Point<Pixels>,
    origin: gpui::Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    size: TerminalSize,
) -> (i32, usize, Side) {
    let cell_w = f32::from(cell_width).max(1.0);
    let relative_x = f32::from(position.x - origin.x).max(0.0);
    let relative_y = f32::from(position.y - origin.y).max(0.0);
    let column = ((relative_x / cell_w) as usize).min(size.columns.saturating_sub(1));
    let row =
        ((relative_y / f32::from(line_height).max(1.0)) as usize).min(size.lines.saturating_sub(1));
    // セル内で左半分なら Left（選択端の丸め。alacritty の選択境界の作法に合わせる）。
    let side = if relative_x - (column as f32) * cell_w < cell_w / 2.0 {
        Side::Left
    } else {
        Side::Right
    };
    (row as i32, column, side)
}

/// ホイールのピクセル増分を行数へ畳む。1 行未満は持ち越して次回に足す
/// （トラックパッドの細かい増分でも取りこぼさず、素早く回せば行が進む）。
/// 戻りは (今回消費する行数, 新しい持ち越し)。
fn wheel_lines(carry: f32, delta_y_pixels: f32, line_height: f32) -> (i32, f32) {
    let total = carry + delta_y_pixels / line_height.max(1.0);
    let lines = total.trunc() as i32;
    (lines, total - lines as f32)
}

impl EventEmitter<TerminalEvent> for TerminalView {}

/// IME 対応の最小実装（M13）: 確定文字列を PTY へ流す。変換中（marked）の
/// インライン表示は持たない＝候補ウィンドウはシステム側で出る。確定時のみ書き込む。
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let units: Vec<u16> = self.marked_text.encode_utf16().collect();
        let text = String::from_utf16(units.get(range_utf16.clone())?).ok()?;
        *actual_range = Some(range_utf16);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // 空選択（カーソル位置相当）。これが Some でないと IME セッションが始まらない。
        Some(UTF16Selection {
            range: self.marked_selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        (!self.marked_text.is_empty()).then(|| 0..self.marked_text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_text.clear();
        self.marked_selection = 0..0;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text.clear();
        self.marked_selection = 0..0;
        // IME 確定・ディクテーション等の文字列を PTY へ（ASCII 打鍵は on_key_down 経由）。
        if !new_text.is_empty() {
            self.scroll_to_bottom(cx);
            self.write_bytes(new_text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // marked_text_range が None だと OS は確定 Enter を通常入力として扱ってしまう（#3）。
        self.marked_text = new_text.to_owned();
        let length = new_text.encode_utf16().count();
        self.marked_selection = new_selected_range_utf16.unwrap_or(length..length);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // IME 候補ウィンドウの位置 = カーソルセルの位置（セル幅は概算で十分）。
        let cursor = self.content.cursor?;
        let cell_width = px(FONT_SIZE * 0.6);
        let origin = element_bounds.origin
            + point(
                cell_width * (cursor.column.0 as f32),
                px(LINE_HEIGHT) * (cursor.line.0 as f32),
            );
        Some(Bounds::new(origin, size(cell_width, px(LINE_HEIGHT))))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
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
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            // テキストを選択できることを示す I ビーム。
            .cursor(CursorStyle::IBeam)
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
            // 端末は等幅必須。UI フォント（IBM Plex Sans JP）を継承すると 1 文字ずつ間延びして
            // 崩れるので、エディタと同じコードフォント（等幅）を明示する。要素は text_style().font()
            // を読むのでコンテナで指定すれば伝播する。
            .font_family("Guguru Sans Code")
            .child(TerminalElement {
                terminal: cx.entity(),
            })
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
            rgb_hsla(
                convert(value / 36),
                convert((value / 6) % 6),
                convert(value % 6),
            )
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
    /// マウス選択の範囲（グリッド座標）。ハイライトの矩形塗りに使う。
    selection: Option<SelectionRange>,
    /// スクロールバックの表示オフセット。グリッド座標 → 表示行は `line + display_offset`。
    display_offset: usize,
    cell_width: Pixels,
    line_height: Pixels,
    origin: gpui::Point<Pixels>,
    focused: bool,
    theme: Theme,
    /// file:line リンク（グリッド行, セル列範囲, パス, 行番号・M13）。
    links: Vec<(i32, Range<usize>, String, u32)>,
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
        let columns = (f32::from(bounds.size.width) / f32::from(cell_width))
            .floor()
            .max(2.0) as usize;
        let lines = (f32::from(bounds.size.height) / f32::from(line_height))
            .floor()
            .max(1.0) as usize;
        term_probe(
            "layout",
            format_args!(
                "bounds {}x{} / cell {}x{} → {columns}列 {lines}行",
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                f32::from(cell_width),
                f32::from(line_height),
            ),
        );
        let theme = self.terminal.read(cx).theme.clone();
        let (cells, cursor, selection, display_offset, focused) = {
            let focused = self.terminal.read(cx).focus_handle.is_focused(window);
            self.terminal.update(cx, |terminal, _cx| {
                terminal.resize(columns, lines);
                (
                    terminal.content.cells.clone(),
                    terminal.content.cursor,
                    terminal.content.selection,
                    terminal.content.display_offset,
                    focused,
                )
            })
        };

        let links = detect_links(&cells);
        TerminalPrepaint {
            cells,
            cursor,
            selection,
            display_offset,
            cell_width,
            line_height,
            origin: bounds.origin,
            focused,
            theme,
            links,
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
        let display_offset = prepaint.display_offset;
        // グリッド行（スクロールバック閲覧中は負値もある）→ 表示 y 座標。
        let row_y = |line: i32| origin.y + line_height * ((line + display_offset as i32) as f32);
        let font = window.text_style().font();

        // 面全体の背景。
        window.paint_quad(fill(bounds, theme.bg1));

        // ⓪ 選択ハイライト（エディタと同じ選択面色）。背景セルより下に敷く。
        if let Some(range) = prepaint.selection {
            for cell in &prepaint.cells {
                if range.contains(cell.point) {
                    let position = point(
                        origin.x + cell_width * (cell.point.column.0 as f32),
                        row_y(cell.point.line.0),
                    );
                    window.paint_quad(fill(
                        Bounds::new(position, size(cell_width, line_height)),
                        theme_core::editor_selection(),
                    ));
                }
            }
        }

        // ① 既定でない背景セルの矩形。
        for cell in &prepaint.cells {
            let (mut foreground, mut background) = (cell.fg, cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            if !is_default_background(background) {
                let position = point(
                    origin.x + cell_width * (cell.point.column.0 as f32),
                    row_y(cell.point.line.0),
                );
                window.paint_quad(fill(
                    Bounds::new(position, size(cell_width, line_height)),
                    ansi_to_hsla(background, theme),
                ));
            }
        }

        // ② カーソル（focus 時は塗りブロック・非focus は輪郭）。
        // スクロールバック閲覧中は現在行が下へはみ出すので、面内にある時だけ描く。
        if let Some(cursor) = prepaint.cursor {
            let position = point(
                origin.x + cell_width * (cursor.column.0 as f32),
                row_y(cursor.line.0),
            );
            let inside = position.y + line_height <= bounds.origin.y + bounds.size.height;
            if inside {
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
        }

        // ③ セル文字（1 セル 1 shape。v1 はバッチ無し）。
        for cell in &prepaint.cells {
            if cell.character == ' '
                || cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                || cell.flags.contains(Flags::HIDDEN)
            {
                continue;
            }
            let (mut foreground, mut background) = (cell.fg, cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            // カーソル下の文字は視認性のため背景色で描く。
            let on_cursor =
                prepaint.focused && prepaint.cursor.is_some_and(|cursor| cursor == cell.point);
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
            // file:line リンク範囲は薄い下線でクリック可能を示す（M13）。
            let linked = prepaint.links.iter().any(|(line, columns, _, _)| {
                *line == cell.point.line.0 && columns.contains(&(cell.point.column.0 as usize))
            });
            let run = TextRun {
                len: cell.character.len_utf8(),
                font: cell_font,
                color,
                background_color: None,
                underline: linked.then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(theme.fg2),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let position = point(
                origin.x + cell_width * (cell.point.column.0 as f32),
                row_y(cell.point.line.0),
            );
            let shaped = window.text_system().shape_line(
                SharedString::from(cell.character.to_string()),
                px(FONT_SIZE),
                &[run],
                Some(cell_width),
            );
            if let Err(error) = shaped.paint(
                position,
                line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            ) {
                eprintln!("ターミナル文字描画に失敗: {error}");
            }
        }

        // ④ file:line リンクのクリック（M13）。paint 毎に登録＝このフレームの座標で判定。
        if !prepaint.links.is_empty() {
            let links = prepaint.links.clone();
            let terminal = self.terminal.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !bounds.contains(&event.position)
                {
                    return;
                }
                let column =
                    (f32::from(event.position.x - origin.x) / f32::from(cell_width)) as usize;
                // 表示行 → グリッド行（スクロールバック閲覧中はオフセットぶんずれる）。
                let row = (f32::from(event.position.y - origin.y) / f32::from(line_height)) as i32
                    - display_offset as i32;
                for (line, columns, path, line_number) in &links {
                    if *line == row && columns.contains(&column) {
                        let (path, line_number) = (path.clone(), *line_number);
                        terminal.update(cx, |_, cx| {
                            cx.emit(TerminalEvent::OpenPath {
                                path,
                                line: line_number,
                            });
                        });
                        break;
                    }
                }
            });
        }

        // ④.5 マウスによるテキスト選択（down=アンカー / move=延長 / up=確定 → ⌘C でコピー）。
        // 座標は file:line リンクと同じ origin/cell_width/line_height を使う。
        {
            let terminal = self.terminal.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !bounds.contains(&event.position)
                {
                    return;
                }
                terminal.update(cx, |terminal, cx| {
                    terminal.begin_selection(event.position, origin, cell_width, line_height, cx);
                });
            });
        }
        {
            let terminal = self.terminal.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || event.pressed_button != Some(MouseButton::Left)
                {
                    return;
                }
                terminal.update(cx, |terminal, cx| {
                    terminal.update_selection(event.position, origin, cell_width, line_height, cx);
                });
            });
        }
        {
            let terminal = self.terminal.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                terminal.update(cx, |terminal, cx| terminal.end_selection(cx));
            });
        }

        // ⑤ IME 入力ハンドラ（M13）: 日本語などの確定文字列を PTY へ流せるようにする。
        window.handle_input(
            &self.terminal.read(cx).focus_handle,
            ElementInputHandler::new(bounds, self.terminal.clone()),
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 行ぶんの表示セルを組む（列は 0 から連番・全角は WIDE_CHAR_SPACER を挟む）。
    fn row_cells(line: i32, text: &str) -> Vec<RenderCell> {
        let mut cells = Vec::new();
        let mut column = 0usize;
        for character in text.chars() {
            let wide = !character.is_ascii();
            cells.push(RenderCell {
                point: AlacPoint::new(alacritty_terminal::index::Line(line), Column(column)),
                character,
                fg: AnsiColor::Named(NamedColor::Foreground),
                bg: AnsiColor::Named(NamedColor::Background),
                flags: Flags::empty(),
            });
            column += 1;
            if wide {
                cells.push(RenderCell {
                    point: AlacPoint::new(alacritty_terminal::index::Line(line), Column(column)),
                    character: ' ',
                    fg: AnsiColor::Named(NamedColor::Foreground),
                    bg: AnsiColor::Named(NamedColor::Background),
                    flags: Flags::WIDE_CHAR_SPACER,
                });
                column += 1;
            }
        }
        cells
    }

    /// 前面のグループがシェル自身なら「動いていない」、別のグループ（ジョブ）なら「動いている」。
    #[cfg(unix)]
    #[test]
    fn a_foreground_job_counts_only_when_it_is_not_the_shell() {
        assert!(!foreground_is_busy(4242, 4242), "プロンプト待ちのシェル");
        assert!(foreground_is_busy(4343, 4242), "前面でジョブが動いている");
        assert!(!foreground_is_busy(-1, 4242), "読めない時は確認しない側");
        assert!(!foreground_is_busy(0, 4242));
    }

    /// 実 PTY で: プロンプト待ちのシェルは「動いていない」、前面で `sleep` が走る間は「動いている」。
    /// `tcgetpgrp` を master 側の複製 fd へ投げて読めること（macOS / Linux）の裏取り。
    ///
    /// 出力は本番と同じく EventLoop に読ませる。誰も読まないと、シェルが終わる時に tty の出力待ちで
    /// 止まり、PTY の後始末（SIGHUP → wait）が返らない。後始末は Shutdown を投げるだけで待たない
    /// （`tests/pty_smoke.rs` と同じ）。
    #[cfg(unix)]
    #[test]
    fn a_real_pty_reports_a_running_foreground_job() {
        use std::time::{Duration, Instant};

        let options = tty::Options {
            // 対話シェル＝ジョブ制御が有効（前面のコマンドは別のプロセスグループで走る）。
            shell: Some(tty::Shell::new(
                "/bin/sh".to_string(),
                vec!["-i".to_string()],
            )),
            working_directory: None,
            drain_on_exit: true,
            env: HashMap::from([("TERM".to_string(), "xterm-256color".to_string())]),
            ..Default::default()
        };
        let window_size = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 8,
            cell_height: 16,
        };
        let (events_tx, _events_rx) = unbounded::<AlacEvent>();
        let listener = Listener(events_tx);
        let size = TerminalSize {
            columns: 80,
            lines: 24,
        };
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &size,
            listener.clone(),
        )));
        let pty = tty::new(&options, window_size, 0).expect("PTY を作れる");
        let probe = ForegroundProbe::new(&pty).expect("master を複製できる");
        let event_loop =
            EventLoop::new(term, listener, pty, true, false).expect("EventLoop を作れる");
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        let wait_until = |want: bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if probe.has_foreground_job() == want {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        assert!(wait_until(false), "起動直後のシェルは前面ジョブ無し");
        notifier.notify(b"sleep 5\n".to_vec());
        let busy = wait_until(true);
        // 先に後始末を投げてから判定する（失敗しても PTY を残さない）。
        if let Err(error) = notifier.0.send(Msg::Shutdown) {
            eprintln!("EventLoop へ Shutdown を送れない: {error}");
        }
        assert!(busy, "sleep が前面で動いている間は true");
    }

    #[test]
    fn detect_links_maps_byte_ranges_to_cell_columns() {
        // cargo/rustc 形式。クリック域は `:col` まで（検出そのものの規則は `ui::links` の test）。
        let cells = row_cells(0, "  --> src/main.rs:10:5");
        let links = detect_links(&cells);
        assert_eq!(links.len(), 1);
        let (line, columns, path, line_number) = &links[0];
        assert_eq!(*line, 0);
        assert_eq!(path, "src/main.rs");
        assert_eq!(*line_number, 10);
        assert_eq!(*columns, 6..22);

        // 全角が先にある行でも列がずれない（byte 範囲 → char 添字 → 列の変換）。
        let cells = row_cells(1, "エラー lib.rs:42");
        let links = detect_links(&cells);
        assert_eq!(links.len(), 1);
        let (_, columns, path, line_number) = &links[0];
        assert_eq!(path, "lib.rs");
        assert_eq!(*line_number, 42);
        // 「エラー」= 3 文字 × 2 列 + 空白 1 列 = 7 列目から。
        assert_eq!(*columns, 7..16);
    }

    #[test]
    fn detect_links_ignores_paths_without_line_numbers() {
        // ターミナルは実在確認をしないので、行番号の無いトークンはリンクにしない
        // （`ls` の出力を全部下線にしない）。
        assert!(detect_links(&row_cells(0, "Cargo.toml  README.md")).is_empty());
    }

    #[test]
    fn wheel_lines_accumulates_fractions() {
        // 1 行未満は持ち越し、合計が 1 行に達した時だけ行が進む（トラックパッドの細かい増分）。
        let (lines, carry) = wheel_lines(0.0, LINE_HEIGHT * 0.6, LINE_HEIGHT);
        assert_eq!(lines, 0);
        let (lines, carry) = wheel_lines(carry, LINE_HEIGHT * 0.6, LINE_HEIGHT);
        assert_eq!(lines, 1);
        assert!(carry.abs() < 1.0);
        // 下方向（負の増分）も対称に畳まれる。
        let (lines, _) = wheel_lines(0.0, -LINE_HEIGHT * 2.5, LINE_HEIGHT);
        assert_eq!(lines, -2);
    }

    #[test]
    fn scroll_display_moves_into_scrollback() {
        // scrolling_history 設定の Term は、出力後に Scroll::Delta で履歴へ遡れて Bottom で戻る。
        // （ホイール修正の土台 API の検証。vte parser 経由で実出力と同じ経路を通す。）
        let (events_tx, _events_rx) = unbounded::<AlacEvent>();
        let listener = Listener(events_tx);
        let size = TerminalSize {
            columns: 80,
            lines: 24,
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        let mut term = Term::new(config, &size, listener);
        let mut parser = alacritty_terminal::vte::ansi::Processor::<
            alacritty_terminal::vte::ansi::StdSyncHandler,
        >::new();
        for index in 0..100 {
            parser.advance(&mut term, format!("行 {index}\r\n").as_bytes());
        }
        assert_eq!(term.grid().display_offset(), 0);
        term.scroll_display(Scroll::Delta(10));
        assert_eq!(term.grid().display_offset(), 10);
        // 履歴の実量を超えては遡れない（クランプされる）。
        term.scroll_display(Scroll::Delta(10_000));
        assert!(term.grid().display_offset() <= 100);
        term.scroll_display(Scroll::Bottom);
        assert_eq!(term.grid().display_offset(), 0);
    }
}

#[cfg(test)]
mod ime_tests {
    use super::*;

    #[gpui::test]
    fn composition_confirmation_does_not_send_enter(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, window, cx| {
            terminal.replace_and_mark_text_in_range(None, "にほんご", Some(4..4), window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), Some(0..4));
            assert!(terminal.written_input.borrow().is_empty());
            // IME 操作用の Enter が通常キー経路に来ても送らない。
            let enter = KeyDownEvent { keystroke: gpui::Keystroke::parse("enter").unwrap(), is_held: false, prefer_character_input: false };
            terminal.on_key_down(&enter, window, cx);
            assert!(terminal.written_input.borrow().is_empty());
            terminal.replace_text_in_range(None, "日本語", window, cx);
            assert_eq!(&*terminal.written_input.borrow(), "日本語".as_bytes());
            assert_eq!(terminal.marked_text_range(window, cx), None);
            // 変換確定の後に、意図して押した次の Enter は通常どおり送信。
            terminal.on_key_down(&enter, window, cx);
            assert_eq!(&*terminal.written_input.borrow(), "日本語\r".as_bytes());
        });
    }

    #[gpui::test]
    fn cancelled_composition_and_utf16_ranges(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, window, cx| {
            terminal.replace_and_mark_text_in_range(None, "猫🐱", Some(1..3), window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), Some(0..3));
            assert_eq!(terminal.selected_text_range(false, window, cx).unwrap().range, 1..3);
            assert_eq!(terminal.text_for_range(1..3, &mut None, window, cx).as_deref(), Some("🐱"));
            terminal.unmark_text(window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), None);
            assert!(terminal.written_input.borrow().is_empty());
            terminal.replace_and_mark_text_in_range(None, "", None, window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), None);
        });
    }
}
