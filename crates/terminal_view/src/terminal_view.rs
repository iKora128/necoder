//! terminal_view — 統合ターミナル（M8）。`alacritty_terminal` を GPUI に載せる最小実装。
//!
//! 独立実装の設計根拠は `docs/research/git-terminal-lsp-design-notes.md`。設計の要点:
//! - **EventLoop が読取スレッド + vte parser**（自前で書かない）。PTY 出力 → parse → `Term` 更新 →
//!   `EventListener::send_event(Wakeup)`。
//! - **idle 0%**: 出力時のみ Wakeup → pump（`cx.spawn`）→ `sync`（スナップショット + notify）。**タイマー無し**。
//! - 通常キーは `on_key_down`、IME は `EntityInputHandler`。前編集は保持し、確定文字だけ PTY へ送る。
//! - 公開 `alacritty_terminal` API と GPUI API を使った necoder 固有の実装。Zed の terminal code は取り込まない。

mod appearance;
mod dock;
mod element;
mod keys;
mod mouse;
mod pty_guard;
mod search;
pub use appearance::{TerminalAppearance, TerminalColors, TerminalCursor};
pub use dock::{
    QuickCommand, QuickCommands, TerminalDock, TerminalDockEvent, TerminalLaunch, TerminalShell,
};

/// 端末のアクション。keymap の `Terminal` コンテキストから引く（`keymap_core` の既定の末尾）。
/// mac は ⌘、Windows / Linux は Ctrl+Shift（⌃ + 文字はシェルへ届ける）。
pub mod actions {
    gpui::actions!(
        terminal,
        [
            /// 選択をクリップボードへ。
            Copy,
            /// クリップボードを貼り付ける（bracketed paste）。
            Paste,
            /// scrollback を含めて全部選択。
            SelectAll,
            /// 画面と scrollback を消す（いまの行 = プロンプトは残す）。
            Clear,
            /// 端末内を検索。
            Find,
        ]
    );
}

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
use alacritty_terminal::term::search::Match;
use alacritty_terminal::term::{ClipboardType, Config, Osc52, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Rgb};

use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;

use gpui::{
    div, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context, CursorStyle,
    EntityInputHandler, EventEmitter, ExternalPaths, FocusHandle, Focusable, Hsla, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Rgba,
    ScrollWheelEvent, UTF16Selection, Window,
};
use std::ops::Range;
use theme_core::Theme;

/// ターミナル → ホスト（workspace）への通知。
pub enum TerminalEvent {
    /// `path:line` リンクのクリック。パスは端末出力のまま（相対は cwd 基準で解決してもらう）。
    OpenPath { path: String, line: u32 },
    /// URL のクリック（`http(s)://…`・スキーム無しの `localhost:3000` は `http://` を補ったもの・
    /// OSC 8 のハイパーリンク）。開き方はホストが決める。
    OpenUrl(String),
    /// アプリがタイトル（OSC 0 / 2）を変えた。タブの名前を描き直す。
    TitleChanged,
}

/// 端末の中のリンクの行き先。
#[derive(Clone, PartialEq, Eq, Debug)]
enum TerminalLinkTarget {
    Path { path: String, line: u32 },
    Url(String),
}

/// 表示中のリンク 1 つ（グリッド行・セル列範囲・行き先）。
#[derive(Clone, PartialEq, Eq, Debug)]
struct TerminalLink {
    line: i32,
    columns: Range<usize>,
    target: TerminalLinkTarget,
}

impl TerminalLink {
    fn contains(&self, point: AlacPoint) -> bool {
        self.line == point.line.0 && self.columns.contains(&point.column.0)
    }
}

/// OSC 8 のハイパーリンクのうち開くもの（http / https だけ。他のスキームは押しても何もしない）。
fn is_openable_uri(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

/// 表示セルからリンクを拾う。OSC 8（アプリが明示したハイパーリンク）が先で、文字から拾う
/// URL と `path:line` は OSC 8 と重ならない所だけ。
fn detect_links(cells: &[RenderCell], hyperlinks: &[String]) -> Vec<TerminalLink> {
    let mut links: Vec<TerminalLink> = Vec::new();
    for cell in cells {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let Some(uri) = cell
            .hyperlink
            .and_then(|index| hyperlinks.get(index as usize))
            .filter(|uri| is_openable_uri(uri))
        else {
            continue;
        };
        let column = cell.point.column.0;
        let width = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        match links.last_mut() {
            Some(link)
                if link.line == cell.point.line.0
                    && link.columns.end == column
                    && link.target == TerminalLinkTarget::Url(uri.clone()) =>
            {
                link.columns.end = column + width;
            }
            _ => links.push(TerminalLink {
                line: cell.point.line.0,
                columns: column..column + width,
                target: TerminalLinkTarget::Url(uri.clone()),
            }),
        }
    }
    let explicit = links.len();

    let mut rows: std::collections::BTreeMap<i32, Vec<(usize, char)>> =
        std::collections::BTreeMap::new();
    for cell in cells {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        rows.entry(cell.point.line.0)
            .or_default()
            .push((cell.point.column.0, cell.character));
    }
    for (line, row) in rows {
        let text: String = row.iter().map(|(_, character)| *character).collect();
        // 検出は `ui::links`（agent_panel の transcript と共有）。ターミナルは cargo/grep の
        // 出力が相手なので、パスは**行番号つきだけ**をリンクにする（実在確認をしないぶん厳しくする）。
        let offsets: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
        for link in ui::links::find_links(&text) {
            let target = match link.target {
                ui::links::LinkTarget::Path {
                    path,
                    line: Some(line_number),
                    ..
                } => TerminalLinkTarget::Path {
                    path,
                    line: line_number,
                },
                ui::links::LinkTarget::Url(url) => TerminalLinkTarget::Url(url),
                ui::links::LinkTarget::Path { line: None, .. } => continue,
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
            let columns = *start_column..*end_column + 1;
            let overlaps_explicit = links[..explicit].iter().any(|explicit| {
                explicit.line == line
                    && explicit.columns.start < columns.end
                    && columns.start < explicit.columns.end
            });
            if !overlaps_explicit {
                links.push(TerminalLink {
                    line,
                    columns,
                    target,
                });
            }
        }
    }
    links
}

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

/// OSC 52 の書き込みをクリップボードへ。選択（primary）は Linux にだけある。
fn store_clipboard(clipboard: ClipboardType, text: String, cx: &mut Context<TerminalView>) {
    let item = ClipboardItem::new_string(text);
    match clipboard {
        ClipboardType::Clipboard => cx.write_to_clipboard(item),
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        ClipboardType::Selection => cx.write_to_primary(item),
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        ClipboardType::Selection => term_probe("osc52", "primary は無いので捨てた"),
    }
}

/// タブに出すタイトル: 1 行目だけ・制御文字を落とす・長すぎるものは切る。
fn sanitize_title(title: &str) -> String {
    const MAX_CHARACTERS: usize = 80;
    title
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_CHARACTERS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// 端末の設定。scrollback とカーソルの既定は見た目の設定（O25・既定 1 万行・ブロック）から。
/// kitty keyboard を受け付ける・OSC 52 は書き込み（コピー）だけ許す（読み出しはクリップボードの
/// 中身をアプリへ渡すことになるので既定で拒否）。
fn terminal_config(appearance: &TerminalAppearance) -> Config {
    Config {
        scrolling_history: appearance.scrollback,
        default_cursor_style: appearance.cursor_style(),
        kitty_keyboard: true,
        osc52: Osc52::OnlyCopy,
        ..Config::default()
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
    /// 下線の色（SGR 58）。None は文字色。
    underline_color: Option<AnsiColor>,
    /// OSC 8 のハイパーリンク（`sync` で集めた URI の一覧の添字）。
    hyperlink: Option<u32>,
}

/// term をロックして取り出した表示スナップショット（前景でロック無しに読める）。
#[derive(Default)]
struct TerminalContent {
    cells: Vec<RenderCell>,
    cursor: Option<AlacPoint>,
    /// カーソルの形（DECSCUSR）。Hidden は描かない（アプリが隠した）。
    cursor_shape: CursorShape,
    /// 表示中のリンク（OSC 8・URL・`path:line`）。描画の下線とクリックの判定に使う。
    links: Vec<TerminalLink>,
    /// マウス選択の範囲（グリッド座標）。ハイライト描画と ⌘C コピーに使う。
    selection: Option<SelectionRange>,
    /// スクロールバックの表示オフセット（0 = 最下段）。セルはグリッド座標のまま持つので、
    /// 描画時に `line + display_offset` で表示行へ写す。
    display_offset: usize,
    /// 検索（⌘F）の一致のうち表示範囲にあるもの。
    search_matches: Vec<Match>,
    /// 検索でいま見ている一致。
    current_match: Option<Match>,
}

/// 描画フレームのグリッドの座標（左上・セルの寸法）。マウスの位置 → セルの変換に使う。
#[derive(Clone, Copy)]
struct GridFrame {
    origin: gpui::Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
}

/// CLI（`necoder terminal read`）向けの画面の写し。行末の空白は落としてある。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalScreen {
    /// 画面より上の scrollback（古い順）。
    pub scrollback: Vec<String>,
    /// 画面に見えている行（上から）。
    pub visible: Vec<String>,
    /// カーソルの位置（画面の行・列。0 始まり）。
    pub cursor_line: usize,
    pub cursor_column: usize,
    pub exited: bool,
}

/// term の grid から画面と scrollback を文字にする。`Line(0..)` は画面の先頭から（ユーザーの
/// スクロール位置に依らない）、`Line(-n)` は scrollback。全角の後ろの詰め物セルは飛ばす。
fn screen_of<T>(term: &Term<T>, scrollback: usize, exited: bool) -> TerminalScreen {
    let grid = term.grid();
    let columns = grid.columns();
    let row_text = |line: Line| {
        let row = &grid[line];
        let mut text = String::new();
        for column in 0..columns {
            let cell = &row[Column(column)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            text.push(cell.c);
            if let Some(zerowidth) = cell.zerowidth() {
                text.extend(zerowidth.iter());
            }
        }
        text.trim_end().to_string()
    };
    let history = scrollback.min(grid.history_size());
    let cursor = grid.cursor.point;
    TerminalScreen {
        scrollback: (1..=history)
            .rev()
            .map(|offset| row_text(Line(-(offset as i32))))
            .collect(),
        visible: (0..grid.screen_lines())
            .map(|line| row_text(Line(line as i32)))
            .collect(),
        cursor_line: cursor.line.0.max(0) as usize,
        cursor_column: cursor.column.0,
        exited,
    }
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
    /// 端末のモード（APP_CURSOR・kitty keyboard のフラグ・マウス報告など）。sync のたびに写す。
    mode: TermMode,
    /// セルの寸法（px）。PTY の WindowSize に載せる（描画のたびに実寸で更新）。
    cell_pixels: (u16, u16),
    /// OS に変換中の範囲を返すための前編集。PTY には確定するまで送らない。
    marked_text: String,
    marked_selection: Range<usize>,
    #[cfg(any(test, feature = "test-support"))]
    written_input: std::cell::RefCell<Vec<u8>>,
    /// 端末内検索（⌘F）。閉じていれば None。
    search: Option<search::TerminalSearch>,
    /// プロジェクト色（検索欄の枠とキャレット＝エディタの ⌘F バーと同じ位置）。
    accent: Hsla,
    /// 最後に PTY から出力が届いた時刻（CLI の `terminal wait` が「出力が N 秒止まった」を判定する）。
    last_output: std::time::Instant,
    /// マウス左ボタンを押してドラッグ選択中か（move で範囲を延ばす判定）。
    selecting: bool,
    /// ポインタが載っているリンク（`content.links` の添字）。下線を濃くし、指の形にする。
    hovered_link: Option<usize>,
    /// 右クリックメニューを出している位置（窓の座標）。閉じていれば None。
    context_menu: Option<gpui::Point<Pixels>>,
    /// 左ボタンを押したセル（グリッド座標）。離した時に同じセルならクリック＝リンクを開く。
    mouse_down_cell: Option<AlacPoint>,
    /// アプリへ押したと報告したボタン（離した時に同じボタンで報告する）。
    reported_button: Option<mouse::ReportButton>,
    /// 最後に報告したセル（表示座標）。同じセルの中の移動は報告しない。
    last_reported_cell: Option<(usize, usize)>,
    /// 直近の描画フレームのグリッドの座標（ホイールの報告で位置 → セルに使う）。
    grid_frame: Option<GridFrame>,
    /// 最後にアプリへ伝えたフォーカス（FOCUS_IN_OUT・同じ状態を二度送らない）。
    reported_focus: Option<bool>,
    /// フォーカスと窓のアクティブの出入りの購読（最初の描画で張る）。
    focus_subscriptions: Vec<gpui::Subscription>,
    /// 見た目の設定（O25・文字の大きさ・フォント・scrollback・カーソル）。global が変わったら入れ直す。
    appearance: TerminalAppearance,
    _appearance_observer: gpui::Subscription,
    /// ドラッグ選択中の最新ポインタ位置とフレーム座標。ビュー外へ引っ張った時の
    /// 自動スクロール tick が選択の引き直しに使う（up で消える）。
    drag_frame: Option<DragFrame>,
    /// 自動スクロールの tick ループが走っているか（二重 spawn 防止）。
    drag_autoscroll_running: bool,
    /// ホイールの 1 行未満の端数持ち越し（トラックパッドのピクセル増分を行単位に畳む）。
    scroll_remainder: f32,
    exited: bool,
    /// アプリが付けたタイトル（OSC 0 / 2）。
    title: Option<String>,
    /// 人が付けた名前（タブのダブルクリック・O24）。あればアプリのタイトルより先に出す。
    custom_title: Option<String>,
    /// ベルが鳴った（次の描画で、窓が後ろにあれば知らせる）。
    bell_pending: bool,
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
        let appearance = TerminalAppearance::current(cx);
        let term = Arc::new(FairMutex::new(Term::new(
            terminal_config(&appearance),
            &size,
            listener.clone(),
        )));

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
            // kitty keyboard のスタック溢れで PTY スレッドが落ちるのを防ぐ（`pty_guard`）。
            Ok(pty) => match EventLoop::new(
                term.clone(),
                listener,
                pty_guard::GuardedPty::new(pty),
                true,
                false,
            ) {
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
            mode: TermMode::default(),
            cell_pixels: (8, 16),
            marked_text: String::new(),
            marked_selection: 0..0,
            #[cfg(any(test, feature = "test-support"))]
            written_input: std::cell::RefCell::new(Vec::new()),
            search: None,
            accent: theme.fg2,
            last_output: std::time::Instant::now(),
            selecting: false,
            hovered_link: None,
            context_menu: None,
            mouse_down_cell: None,
            reported_button: None,
            last_reported_cell: None,
            grid_frame: None,
            reported_focus: None,
            focus_subscriptions: Vec::new(),
            appearance,
            _appearance_observer: cx.observe_global::<TerminalAppearance>(Self::apply_appearance),
            drag_frame: None,
            drag_autoscroll_running: false,
            scroll_remainder: 0.0,
            exited,
            title: None,
            custom_title: None,
            bell_pending: false,
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
        let appearance = TerminalAppearance::current(cx);
        let term = Arc::new(FairMutex::new(Term::new(
            terminal_config(&appearance),
            &size,
            listener,
        )));
        Self {
            term,
            notifier: None,
            content: TerminalContent::default(),
            size,
            mode: TermMode::default(),
            cell_pixels: (8, 16),
            marked_text: String::new(),
            marked_selection: 0..0,
            #[cfg(any(test, feature = "test-support"))]
            written_input: std::cell::RefCell::new(Vec::new()),
            search: None,
            accent: theme.fg2,
            last_output: std::time::Instant::now(),
            selecting: false,
            hovered_link: None,
            context_menu: None,
            mouse_down_cell: None,
            reported_button: None,
            last_reported_cell: None,
            grid_frame: None,
            reported_focus: None,
            focus_subscriptions: Vec::new(),
            appearance,
            _appearance_observer: cx.observe_global::<TerminalAppearance>(Self::apply_appearance),
            drag_frame: None,
            drag_autoscroll_running: false,
            scroll_remainder: 0.0,
            exited: false,
            title: None,
            custom_title: None,
            bell_pending: false,
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

    /// プロジェクト色（検索欄の枠・キャレット）。
    pub fn set_accent(&mut self, accent: Hsla, cx: &mut Context<Self>) {
        self.accent = accent;
        cx.notify();
    }

    /// 見た目の設定が変わった（O25）: 文字の大きさとフォントは次の描画から（行列は prepaint で
    /// 測り直して PTY へ伝わる）。scrollback とカーソルの既定は term に入れ直す（遡れる行を減らした
    /// 時は古い方から捨てる）。
    fn apply_appearance(&mut self, cx: &mut Context<Self>) {
        let next = TerminalAppearance::current(cx);
        if next == self.appearance {
            return;
        }
        if next.scrollback != self.appearance.scrollback || next.cursor != self.appearance.cursor {
            self.term.lock().set_options(terminal_config(&next));
        }
        self.appearance = next;
        cx.notify();
    }

    /// いまの見た目（テスト用）。
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn debug_appearance(&self) -> &TerminalAppearance {
        &self.appearance
    }

    /// 端末内検索のバーが開いているか。
    pub fn search_open(&self) -> bool {
        self.search.is_some()
    }

    /// テスト用: PTY へ書いたバイト列（PTY を起動しない端末でも記録する）。
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn debug_written_input(&self) -> Vec<u8> {
        self.written_input.borrow().clone()
    }

    /// 開発用（offscreen 検証・`NECODER_TERMINAL_PROBE`）: 端末への 1 コマンド。
    /// `type:<文>` = 文をタイプして ⏎ / `select:<行>,<列>-<行>,<列>` = 表示座標で選択 /
    /// `find:<語>` = ⌘F を開いて語を入れる / `key:<キー>` = キーを 1 つ打つ（`shift-enter`）/
    /// `menu:<x>,<y>` = 右クリックメニューを出す。
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_probe(
        &mut self,
        name: &str,
        argument: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match name {
            "type" => {
                let mut bytes = argument.as_bytes().to_vec();
                bytes.push(b'\r');
                self.write_bytes(bytes);
            }
            "select" => {
                let parse = |point: &str| {
                    let (row, column) = point.split_once(',')?;
                    Some((
                        row.trim().parse::<i32>().ok()?,
                        column.trim().parse::<usize>().ok()?,
                    ))
                };
                let Some(((start_row, start_column), (end_row, end_column))) = argument
                    .split_once('-')
                    .and_then(|(start, end)| Some((parse(start)?, parse(end)?)))
                else {
                    eprintln!("TERMINAL_PROBE: select の形が不正: {argument:?}");
                    return;
                };
                let mut term = self.term.lock();
                let offset = term.grid().display_offset() as i32;
                let start = AlacPoint::new(Line(start_row - offset), Column(start_column));
                let end = AlacPoint::new(Line(end_row - offset), Column(end_column));
                let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
                selection.update(end, Side::Right);
                term.selection = Some(selection);
                drop(term);
                self.sync(cx);
            }
            "find" => {
                self.find(&actions::Find, window, cx);
                if let Some(search) = self.search.as_mut() {
                    search.query = argument.to_string();
                }
                self.refresh_search(true, cx);
            }
            // キーを 1 つ打つ（`shift-enter` など・実際の打鍵と同じ符号化を通す）。
            "key" => match gpui::Keystroke::parse(argument) {
                Ok(keystroke) => {
                    if let Some(bytes) = keys::keystroke_to_bytes(&keystroke, self.mode, false) {
                        self.write_bytes(bytes);
                    }
                }
                Err(error) => eprintln!("TERMINAL_PROBE: key が不正: {error}"),
            },
            // 右クリックメニューを窓の座標 (x, y) に出す。
            "menu" => {
                let Some((x, y)) = argument.split_once(',').and_then(|(x, y)| {
                    Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
                }) else {
                    eprintln!("TERMINAL_PROBE: menu の形が不正: {argument:?}");
                    return;
                };
                self.context_menu = Some(point(px(x), px(y)));
                cx.notify();
            }
            other => eprintln!("TERMINAL_PROBE: 未知のコマンド {other}"),
        }
    }

    /// alacritty イベントを処理する（前景・pump から）。Wakeup で再スナップショット。
    fn on_alac_event(&mut self, event: AlacEvent, cx: &mut Context<Self>) {
        match event {
            AlacEvent::Wakeup => {
                self.last_output = std::time::Instant::now();
                self.sync(cx);
                // 検索中は出力が増えた分を数え直す（間引いて）。
                self.schedule_search_refresh(cx);
            }
            AlacEvent::Exit => {
                self.exited = true;
                cx.notify();
            }
            // アプリがPTYへ書き戻しを要求（端末問い合わせ応答等）。
            AlacEvent::PtyWrite(text) => self.write_bytes(text.into_bytes()),
            // OSC 52 の書き込み（`tmux` / `nvim` / SSH 先のアプリのコピー）。
            AlacEvent::ClipboardStore(clipboard, text) => store_clipboard(clipboard, text, cx),
            // OSC 52 の読み出しはクリップボードの中身をアプリへ渡すことになるので応えない
            // （Config.osc52 = OnlyCopy なので alacritty も普通は出さない）。
            AlacEvent::ClipboardLoad(..) => term_probe("osc52", "読み出しの要求を断った"),
            AlacEvent::Title(title) => self.set_title(Some(title), cx),
            AlacEvent::ResetTitle => self.set_title(None, cx),
            // ベル: 窓が後ろにある時だけ知らせる（描画の時に窓へ頼む）。
            AlacEvent::Bell => {
                self.bell_pending = true;
                cx.notify();
            }
            // OSC 4 / 10 / 11 / 12 の色の問い合わせ（TUI が背景の明暗を見て配色を決める）。
            AlacEvent::ColorRequest(index, format) => {
                let rgb = self.color_for_request(index);
                self.write_bytes(format(rgb).into_bytes());
            }
            // `CSI 14 t`（文字領域の px）の問い合わせ。
            AlacEvent::TextAreaSizeRequest(format) => {
                let size = self.window_size();
                self.write_bytes(format(size).into_bytes());
            }
            AlacEvent::MouseCursorDirty
            | AlacEvent::CursorBlinkingChange
            | AlacEvent::ChildExit(_) => {}
        }
    }

    /// アプリが付けたタイトル（OSC 0 / 2）。無ければ None（タブは「ターミナル N」）。
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// 人が付けた名前（無ければ `None`）。
    pub fn custom_title(&self) -> Option<&str> {
        self.custom_title.as_deref()
    }

    /// タブに出す名前: 人が付けた名前 → アプリのタイトル（OSC 0 / 2）の順。
    pub fn display_title(&self) -> Option<&str> {
        self.custom_title.as_deref().or(self.title.as_deref())
    }

    /// 人が付けた名前を置く（空・空白だけ = 外して、アプリのタイトルに戻す・O24）。
    pub fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        let title = title
            .map(|title| sanitize_title(&title))
            .filter(|title| !title.is_empty());
        if self.custom_title != title {
            self.custom_title = title;
            cx.emit(TerminalEvent::TitleChanged);
            cx.notify();
        }
    }

    fn set_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        let title = title
            .map(|title| sanitize_title(&title))
            .filter(|title| !title.is_empty());
        if self.title != title {
            self.title = title;
            cx.emit(TerminalEvent::TitleChanged);
            cx.notify();
        }
    }

    /// 色の問い合わせへの答え。アプリが OSC で上書きした色があればそれ、無ければ描画と同じ色
    /// （0〜255 = パレット・256 = 文字・257 = 背景・258 = カーソル）。
    fn color_for_request(&self, index: usize) -> Rgb {
        let overridden = (index < alacritty_terminal::term::color::COUNT)
            .then(|| self.term.lock().colors()[index])
            .flatten();
        if let Some(rgb) = overridden {
            return rgb;
        }
        let colors = &self.appearance.colors;
        let color = match index {
            0..=255 => indexed_to_hsla(index as u8, colors),
            257 => background_hsla(&self.theme, colors),
            258 => self.theme.fg1,
            _ => foreground_hsla(&self.theme, colors),
        };
        let rgba = Rgba::from(color);
        let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
        Rgb {
            r: channel(rgba.r),
            g: channel(rgba.g),
            b: channel(rgba.b),
        }
    }

    /// term をロックして表示スナップショットを取り直し、再描画を促す。
    fn sync(&mut self, cx: &mut Context<Self>) {
        let term = self.term.lock();
        let content = term.renderable_content();
        let display_offset = content.display_offset;
        let cursor_shape = content.cursor.shape;
        let mut hyperlinks: Vec<String> = Vec::new();
        let cells: Vec<RenderCell> = content
            .display_iter
            .map(|indexed| {
                // 同じ URI は 1 つにまとめて添字で指す（リンクの区切りの判定にも使う）。
                let hyperlink = indexed.cell.hyperlink().map(|link| {
                    let uri = link.uri();
                    match hyperlinks.iter().rposition(|known| known == uri) {
                        Some(index) => index as u32,
                        None => {
                            hyperlinks.push(uri.to_string());
                            (hyperlinks.len() - 1) as u32
                        }
                    }
                });
                RenderCell {
                    point: indexed.point,
                    character: indexed.cell.c,
                    fg: indexed.cell.fg,
                    bg: indexed.cell.bg,
                    flags: indexed.cell.flags,
                    underline_color: indexed.cell.underline_color(),
                    hyperlink,
                }
            })
            .collect();
        let cursor = Some(content.cursor.point);
        self.mode = *term.mode();
        // 選択範囲もスナップショットに含める（前景でロック無しにハイライトを描くため）。
        let selection = term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&term));
        // 検索の一致は表示範囲だけ探し直す（出力で行が流れても強調の位置がずれない）。
        let (search_matches, current_match) = match self.search.as_mut() {
            Some(search) => (
                search
                    .regex
                    .as_mut()
                    .map(|regex| search::find_visible(&term, regex))
                    .unwrap_or_default(),
                search
                    .current
                    .and_then(|index| search.matches.get(index).cloned()),
            ),
            None => (Vec::new(), None),
        };
        drop(term);
        let links = detect_links(&cells, &hyperlinks);
        // 描き直しでリンクの並びが変わったら、ホバー中の添字は捨てる（次の move で拾い直す）。
        if links != self.content.links {
            self.hovered_link = None;
        }
        self.content = TerminalContent {
            cells,
            cursor,
            cursor_shape,
            links,
            selection,
            display_offset,
            search_matches,
            current_match,
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
        #[cfg(any(test, feature = "test-support"))]
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

    /// CLI（`necoder terminal send`）からの入力をそのまま PTY へ送る。`insert_text` と違い改行も送る
    /// （`\n` は端末の Enter = `\r` に直す）。許可の判断は呼び手（GUI の設定）がする。
    pub fn send_input(&self, text: &str) {
        let bytes = text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
        if !bytes.is_empty() {
            self.write_bytes(bytes);
        }
    }

    /// 最後に出力が届いてからの時間（CLI の `terminal wait`）。
    pub fn output_idle(&self) -> std::time::Duration {
        self.last_output.elapsed()
    }

    /// シェルが終わっている（PTY を作れなかった端末を含む）。
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// 画面に見えている行と、その上の scrollback（最大 `scrollback` 行）を文字で読む
    /// （CLI の `terminal read`）。ユーザーがスクロールで遡っていても、読むのは今の画面。
    pub fn read_screen(&self, scrollback: usize) -> TerminalScreen {
        screen_of(&self.term.lock(), scrollback, self.exited)
    }

    /// テストで PTY の出力の代わりにバイト列を流し込む（vte を通すので本物の出力と同じ経路で画面に載る）。
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn feed_output_for_test(&mut self, bytes: &[u8]) {
        let mut parser = alacritty_terminal::vte::ansi::Processor::<
            alacritty_terminal::vte::ansi::StdSyncHandler,
        >::new();
        parser.advance(&mut *self.term.lock(), bytes);
        self.last_output = std::time::Instant::now();
    }

    /// テストで、この端末へ送られた入力を読む。
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn written_input_for_test(&self) -> Vec<u8> {
        self.written_input.borrow().clone()
    }

    /// 行列サイズ（とセルの寸法）が変わったら term と PTY をリサイズする（prepaint から）。
    fn resize(&mut self, columns: usize, lines: usize, cell_width: Pixels, line_height: Pixels) {
        let new_size = TerminalSize {
            columns: columns.max(2),
            lines: lines.max(1),
        };
        // PTY へ伝えるセルの寸法（px・`CSI 14 t` の応答や画像を出すアプリが使う）。
        let cell_pixels = (
            f32::from(cell_width).round().max(1.0) as u16,
            f32::from(line_height).round().max(1.0) as u16,
        );
        if new_size == self.size && cell_pixels == self.cell_pixels {
            return;
        }
        let size_changed = new_size != self.size;
        self.size = new_size;
        self.cell_pixels = cell_pixels;
        if size_changed {
            self.term.lock().resize(new_size);
        }
        if let Some(notifier) = &self.notifier {
            let window_size = self.window_size();
            if let Err(error) = notifier.0.send(Msg::Resize(window_size)) {
                eprintln!("ターミナル: リサイズ送信に失敗: {error}");
            }
        }
    }

    /// PTY へ伝える大きさ（行列 + セルの px）。
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.size.lines as u16,
            num_cols: self.size.columns as u16,
            cell_width: self.cell_pixels.0,
            cell_height: self.cell_pixels.1,
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // 変換候補の操作は IME のもの。未処理キーが流れてきても PTY へ漏らさない。
        if !self.marked_text.is_empty() {
            cx.stop_propagation();
            return;
        }
        // 右クリックメニューはキーを押したら閉じる（Esc は閉じるだけで端末へ送らない）。
        if self.context_menu.take().is_some() {
            cx.notify();
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                return;
            }
        }
        // ⌘C / ⌘V / ⌘A / ⌘K / ⌘F は keymap の `Terminal` コンテキスト（actions）が先に受ける。
        if let Some(bytes) = keys::keystroke_to_bytes(&event.keystroke, self.mode, event.is_held) {
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
        let line_height = self.appearance.line_height();
        let delta_y = f32::from(event.delta.pixel_delta(px(line_height)).y);
        let (lines, remainder) = wheel_lines(self.scroll_remainder, delta_y, line_height);
        self.scroll_remainder = remainder;
        if lines == 0 {
            return;
        }
        // アプリがマウスの報告を求めていれば、ホイールも報告する（1 行 = 1 回）。
        if mouse::wants_report(self.mode, event.modifiers) {
            if let Some(frame) = self.grid_frame {
                let cell = self.viewport_point(event.position, frame);
                let button = if lines > 0 {
                    mouse::ReportButton::WheelUp
                } else {
                    mouse::ReportButton::WheelDown
                };
                for _ in 0..lines.unsigned_abs() {
                    self.report_mouse(button, mouse::ReportAction::Press, cell, event.modifiers);
                }
            }
            return;
        }
        let mut term = self.term.lock();
        let mode = *term.mode();
        if mode.contains(TermMode::ALT_SCREEN) {
            drop(term);
            // 代替画面にスクロールバックは無い。ALTERNATE_SCROLL が立っていれば
            // 上=↑ / 下=↓ を行数ぶん送ってアプリ側（less/vim）にスクロールさせる。
            if mode.contains(TermMode::ALTERNATE_SCROLL) {
                let app_cursor = mode.contains(TermMode::APP_CURSOR);
                let code: &[u8] = match (lines > 0, app_cursor) {
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

    /// 文字列を貼り付けとして PTY へ送る（⌘V・ファイルのドロップ・右クリックの貼り付け）。
    fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.scroll_to_bottom(cx);
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        self.write_bytes(paste_bytes(text, bracketed));
    }

    // ── keymap の `Terminal` コンテキストのアクション ──

    fn copy(&mut self, _: &actions::Copy, _window: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    fn paste(&mut self, _: &actions::Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        // 検索欄にフォーカスがある時は語へ入れる（1 行目だけ）。
        if let Some(search) = self.search.as_mut() {
            if search.focus.is_focused(window) {
                search
                    .query
                    .push_str(text.lines().next().unwrap_or_default());
                self.refresh_search(true, cx);
                return;
            }
        }
        self.paste_text(&text, cx);
    }

    fn select_all(&mut self, _: &actions::SelectAll, _window: &mut Window, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        let start = AlacPoint::new(term.topmost_line(), Column(0));
        let end = AlacPoint::new(term.bottommost_line(), term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
        drop(term);
        self.sync(cx);
    }

    fn clear(&mut self, _: &actions::Clear, _window: &mut Window, cx: &mut Context<Self>) {
        self.clear_scrollback(cx);
    }

    /// ⌘K: scrollback と画面を消し、いまの行（プロンプト）を先頭に残す。代替画面（vim / less /
    /// TUI のエージェント）は画面をアプリが持っているので触らない。
    fn clear_scrollback(&mut self, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let screen_lines = term.screen_lines() as i32;
        let cursor_line = term.grid().cursor.point.line.0.clamp(0, screen_lines - 1);
        if cursor_line > 0 {
            // いまの行より上を scrollback へ押し出してから、scrollback ごと捨てる。
            let region = Line(0)..Line(screen_lines);
            term.grid_mut()
                .scroll_up::<AnsiColor>(&region, cursor_line as usize);
            term.grid_mut().cursor.point.line = Line(0);
        }
        term.grid_mut().clear_history();
        term.selection = None;
        drop(term);
        self.sync(cx);
    }

    /// ⌘F。検索バーを開いて入力欄へ。開いていれば入力欄へフォーカスを戻すだけ。
    fn find(&mut self, _: &actions::Find, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            let mut search =
                search::TerminalSearch::new(cx.focus_handle(), self.content.display_offset);
            // 1 行の選択があれば語の初期値に（エディタの ⌘F と同じ）。
            if let Some(text) = self.term.lock().selection_to_string() {
                let text = text.trim_end_matches('\n');
                if !text.is_empty() && !text.contains('\n') && text.len() <= 200 {
                    search.query = text.to_string();
                }
            }
            self.search = Some(search);
            self.refresh_search(true, cx);
        }
        if let Some(search) = &self.search {
            window.focus(&search.focus, cx);
        }
        cx.notify();
    }

    /// 検索バーを閉じる。`restore` = 開いた時の表示位置へ戻す（Esc）。× は今の位置のまま。
    fn close_search(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = self.search.take() else {
            return;
        };
        if restore {
            let mut term = self.term.lock();
            let current = term.grid().display_offset() as i32;
            term.scroll_display(Scroll::Delta(search.saved_offset as i32 - current));
        }
        self.sync(cx);
        window.focus(&self.focus_handle, cx);
    }

    /// 検索を数え直す。`query_changed` = 語かトグルが変わった（正規表現を組み直し、いまの表示に
    /// 一番近い一致へ移る）。false = 出力が増えただけ（見ている一致の番号を保つ）。
    fn refresh_search(&mut self, query_changed: bool, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if query_changed {
            search.rebuild_regex();
        }
        let mut term = self.term.lock();
        match search.regex.as_mut() {
            Some(regex) => {
                let (matches, truncated) = search::find_all(&term, regex);
                search.matches = matches;
                search.truncated = truncated;
            }
            None => {
                search.matches.clear();
                search.truncated = false;
            }
        }
        if query_changed {
            let display_offset = term.grid().display_offset() as i32;
            let bottom = Line(term.screen_lines() as i32 - 1 - display_offset);
            search.current = search::nearest_match(&search.matches, bottom);
            if let Some(found) = search.current.and_then(|index| search.matches.get(index)) {
                term.scroll_to_point(*found.start());
            }
        } else {
            let count = search.matches.len();
            search.current = search
                .current
                .filter(|_| count > 0)
                .map(|index| index.min(count - 1));
        }
        drop(term);
        self.sync(cx);
    }

    /// 次（`delta` = 1・新しい方）/ 前（-1）の一致へ移り、見える位置までスクロールする。
    fn step_search(&mut self, delta: isize, cx: &mut Context<Self>) {
        // 出力で古くなっているかもしれないので、数え直してから動く。
        self.refresh_search(false, cx);
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let count = search.matches.len();
        if count == 0 {
            return;
        }
        let next = match search.current {
            Some(index) => (index as isize + delta).rem_euclid(count as isize) as usize,
            None if delta > 0 => 0,
            None => count - 1,
        };
        search.current = Some(next);
        let point = *search.matches[next].start();
        self.term.lock().scroll_to_point(point);
        self.sync(cx);
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
    /// ダブルクリックは語（`Semantic`）、トリプルクリックは行（`Lines`）の単位で選ぶ。
    fn begin_selection(
        &mut self,
        position: gpui::Point<Pixels>,
        frame: GridFrame,
        selection_type: SelectionType,
        cx: &mut Context<Self>,
    ) {
        let (row, column, side) = viewport_cell(
            position,
            frame.origin,
            frame.cell_width,
            frame.line_height,
            self.size,
        );
        self.selecting = true;
        let mut term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let point = AlacPoint::new(Line(row - offset), Column(column));
        term.selection = Some(Selection::new(selection_type, point, side));
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

impl TerminalView {
    /// ピクセル位置 → グリッド座標（scrollback の表示位置を反映）。
    fn grid_point(&self, position: gpui::Point<Pixels>, frame: GridFrame) -> AlacPoint {
        let (row, column, _) = viewport_cell(
            position,
            frame.origin,
            frame.cell_width,
            frame.line_height,
            self.size,
        );
        AlacPoint::new(
            Line(row - self.content.display_offset as i32),
            Column(column),
        )
    }

    /// ピクセル位置 → 表示座標のセル（列, 行。報告用・0 始まり）。
    fn viewport_point(&self, position: gpui::Point<Pixels>, frame: GridFrame) -> (usize, usize) {
        let (row, column, _) = viewport_cell(
            position,
            frame.origin,
            frame.cell_width,
            frame.line_height,
            self.size,
        );
        (column, row.max(0) as usize)
    }

    /// マウスの報告を 1 つ PTY へ送る。
    fn report_mouse(
        &mut self,
        button: mouse::ReportButton,
        action: mouse::ReportAction,
        cell: (usize, usize),
        modifiers: gpui::Modifiers,
    ) {
        self.last_reported_cell = Some(cell);
        if let Some(bytes) = mouse::encode(button, action, cell.0, cell.1, modifiers, self.mode) {
            self.write_bytes(bytes);
        }
    }

    /// グリッドの上でボタンを押した（`element.rs` の paint が、このフレームの座標で呼ぶ）。
    /// アプリがマウスの報告を求めていれば報告し（⇧ を押している間は選択）、そうでなければ
    /// 左は選択を始め、右はメニューを出す。
    fn on_grid_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        frame: GridFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mouse::wants_report(self.mode, event.modifiers) {
            if let Some(button) = mouse::ReportButton::from_gpui(event.button) {
                let cell = self.viewport_point(event.position, frame);
                self.reported_button = Some(button);
                self.report_mouse(button, mouse::ReportAction::Press, cell, event.modifiers);
            }
            return;
        }
        if event.button == MouseButton::Right {
            window.focus(&self.focus_handle, cx);
            self.context_menu = Some(event.position);
            cx.notify();
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        self.mouse_down_cell = Some(self.grid_point(event.position, frame));
        let selection_type = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.begin_selection(event.position, frame, selection_type, cx);
    }

    /// ポインタが動いた。左ボタンを押していれば選択を延ばし、そうでなければリンクのホバーを追う
    /// （変わった時だけ描き直す＝動かしているだけでは再描画しない）。
    fn on_grid_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        frame: GridFrame,
        inside: bool,
        cx: &mut Context<Self>,
    ) {
        // アプリへの報告中（押したボタンの移動・1003 なら全ての移動）。同じセルの中は送らない。
        // ボタンを押していない移動は端末の上にある時だけ（エディタの上を動かしても送らない）。
        if self.reported_button.is_some()
            || (inside && mouse::wants_report(self.mode, event.modifiers))
        {
            let held = self.reported_button.is_some();
            if mouse::wants_motion(self.mode, held) {
                let cell = self.viewport_point(event.position, frame);
                if self.last_reported_cell != Some(cell) {
                    let button = self
                        .reported_button
                        .unwrap_or(mouse::ReportButton::NoButton);
                    self.report_mouse(button, mouse::ReportAction::Motion, cell, event.modifiers);
                }
            }
            if held {
                return;
            }
        }
        if event.pressed_button == Some(MouseButton::Left) {
            self.update_selection(
                event.position,
                frame.origin,
                frame.cell_width,
                frame.line_height,
                cx,
            );
            return;
        }
        let hovered = if inside {
            let point = self.grid_point(event.position, frame);
            self.content
                .links
                .iter()
                .position(|link| link.contains(point))
        } else {
            None
        };
        if hovered != self.hovered_link {
            self.hovered_link = hovered;
            cx.notify();
        }
    }

    /// ボタンを離した。動かさずに離した（クリック）ならリンクを開く（path:line と URL で同じ操作感）。
    fn on_grid_mouse_up(&mut self, event: &MouseUpEvent, frame: GridFrame, cx: &mut Context<Self>) {
        // 押した時に報告したボタンは、離した時も（⇧ に関わらず）報告して閉じる。
        if let Some(button) = self.reported_button {
            if mouse::ReportButton::from_gpui(event.button) == Some(button) {
                self.reported_button = None;
                let cell = self.viewport_point(event.position, frame);
                self.report_mouse(button, mouse::ReportAction::Release, cell, event.modifiers);
            }
            return;
        }
        if event.button != MouseButton::Left || (!self.selecting && self.mouse_down_cell.is_none())
        {
            return;
        }
        let up = self.grid_point(event.position, frame);
        let clicked = self.mouse_down_cell.take().filter(|down| *down == up);
        let dragged = self
            .term
            .lock()
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.is_empty());
        self.end_selection(cx);
        if let (Some(point), false) = (clicked, dragged) {
            self.open_link_at(point, cx);
        }
    }

    /// そのセルのリンクを開く（ホストへ通知するだけ。開き方はホストが決める）。
    fn open_link_at(&mut self, point: AlacPoint, cx: &mut Context<Self>) {
        let Some(link) = self.content.links.iter().find(|link| link.contains(point)) else {
            return;
        };
        match link.target.clone() {
            TerminalLinkTarget::Path { path, line } => {
                cx.emit(TerminalEvent::OpenPath { path, line })
            }
            TerminalLinkTarget::Url(url) => cx.emit(TerminalEvent::OpenUrl(url)),
        }
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
        // IME 候補ウィンドウの位置 = カーソルセルの位置。描いたフレームがあればその実寸、
        // まだなら文字の大きさからの概算。
        let cursor = self.content.cursor?;
        let (cell_width, line_height) = match self.grid_frame {
            Some(frame) => (frame.cell_width, frame.line_height),
            None => (
                px(self.appearance.font_size * 0.6),
                px(self.appearance.line_height()),
            ),
        };
        let origin = element_bounds.origin
            + point(
                cell_width * (cursor.column.0 as f32),
                line_height * (cursor.line.0 as f32),
            );
        Some(Bounds::new(origin, size(cell_width, line_height)))
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

impl TerminalView {
    /// フォーカスの出入りをアプリへ伝える（FOCUS_IN_OUT・`CSI I` / `CSI O`）。
    /// 端末にフォーカスがあり、かつ窓がアクティブな時を「入っている」とする。
    fn report_focus(&mut self, window: &mut Window) {
        let focused = self.focus_handle.is_focused(window) && window.is_window_active();
        if self.reported_focus == Some(focused) {
            return;
        }
        self.reported_focus = Some(focused);
        if self.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
            let bytes: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.write_bytes(bytes.to_vec());
        }
    }

    /// フォーカスと窓のアクティブの出入りを購読する（窓が要るので最初の描画で張る）。
    fn subscribe_focus_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_subscriptions.is_empty() {
            return;
        }
        // 今の状態は伝えずに覚えるだけ（変わった時だけ伝える＝xterm と同じ）。
        self.reported_focus =
            Some(self.focus_handle.is_focused(window) && window.is_window_active());
        let focus_handle = self.focus_handle.clone();
        self.focus_subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, window, _cx| {
                this.report_focus(window)
            }),
            cx.on_blur(&focus_handle, window, |this, window, _cx| {
                this.report_focus(window)
            }),
            cx.observe_window_activation(window, |this, window, _cx| this.report_focus(window)),
        ];
    }
}

impl TerminalView {
    /// ドロップされたパスを、シェル向けに引用して貼り付けとして入れる（空白区切り・末尾に空白）。
    fn drop_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let mut text = paths
            .iter()
            .map(|path| quote_path_for_shell(&path.to_string_lossy(), cfg!(windows)))
            .collect::<Vec<_>>()
            .join(" ");
        text.push(' ');
        self.paste_text(&text, cx);
        window.focus(&self.focus_handle, cx);
    }

    /// 右クリックメニュー（開いている時だけ）。見た目はタブ / エクスプローラのメニューと同じ部品に、
    /// キーを右に添える。コピーは選択が無い時は押せない。
    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let position = self.context_menu?;
        let theme = self.theme.clone();
        let has_selection = self.content.selection.is_some();
        let item = |id: &'static str, label: String, key: &str, enabled: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .px(px(9.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(if enabled { theme.fg1 } else { theme.fg2 })
                .when(enabled, |element| {
                    element
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                })
                .child(label)
                .child(div().text_size(px(11.)).text_color(theme.fg2).child(
                    keymap_core::keystroke_label_in(keymap_core::TERMINAL_CONTEXT, key),
                ))
        };
        let separator = div().h(px(1.)).bg(theme.border).my(px(3.));
        let menu = div()
            .w(px(220.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            .font_family(ui::ui_font(cx))
            .on_mouse_down_out(cx.listener(|this, _, _window, cx| {
                this.context_menu = None;
                cx.notify();
            }))
            .child(
                item(
                    "terminal-menu-copy",
                    i18n::t!("terminal.menu_copy"),
                    "cmd-c",
                    has_selection,
                )
                .when(has_selection, |element| {
                    element.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            cx.stop_propagation();
                            this.context_menu = None;
                            this.copy_selection(cx);
                            cx.notify();
                        }),
                    )
                }),
            )
            .child(
                item(
                    "terminal-menu-paste",
                    i18n::t!("terminal.menu_paste"),
                    "cmd-v",
                    true,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.context_menu = None;
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            this.paste_text(&text, cx);
                        }
                        cx.notify();
                    }),
                ),
            )
            .child(
                item(
                    "terminal-menu-select-all",
                    i18n::t!("terminal.menu_select_all"),
                    "cmd-a",
                    true,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.context_menu = None;
                        this.select_all(&actions::SelectAll, window, cx);
                    }),
                ),
            )
            .child(separator)
            .child(
                item(
                    "terminal-menu-clear",
                    i18n::t!("terminal.menu_clear"),
                    "cmd-k",
                    true,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.context_menu = None;
                        this.clear_scrollback(cx);
                        cx.notify();
                    }),
                ),
            )
            .child(
                item(
                    "terminal-menu-find",
                    i18n::t!("terminal.menu_find"),
                    "cmd-f",
                    true,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.context_menu = None;
                        this.find(&actions::Find, window, cx);
                    }),
                ),
            );
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(position)
                    .snap_to_window_with_margin(px(8.))
                    .child(menu),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

/// シェルに打つためのパスの引用。安全な文字だけならそのまま、それ以外は
/// unix（sh / zsh / bash）は単引用符（中の `'` は `'\''`）、Windows（pwsh / cmd）は二重引用符。
fn quote_path_for_shell(path: &str, windows: bool) -> String {
    let safe = !path.is_empty()
        && path.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '/' | '.' | '_' | '-' | '+' | '@' | '%' | ':' | ',' | '='
                )
                || (windows && character == '\\')
        });
    if safe {
        path.to_string()
    } else if windows {
        format!("\"{path}\"")
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.subscribe_focus_changes(window, cx);
        // ベル: 窓が後ろにある時だけ知らせる（Dock のアイコンが跳ねる程度）。前にある時は何もしない。
        if std::mem::take(&mut self.bell_pending)
            && !window.is_window_active()
            && self.mode.contains(TermMode::URGENCY_HINTS)
        {
            window.request_attention();
        }
        let search_bar = self.render_search_bar(window, cx);
        let context_menu = self.render_context_menu(cx);
        div()
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::find))
            // Finder などからのファイル / エクスプローラの行のドロップ: 引用したパスを貼り付ける。
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.drop_paths(paths.paths(), window, cx)
            }))
            .on_drop(cx.listener(|this, dragged: &ui::DraggedFile, window, cx| {
                this.drop_paths(std::slice::from_ref(&dragged.source), window, cx)
            }))
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
            .relative()
            .size_full()
            .bg(background_hsla(&self.theme, &self.appearance.colors))
            // 端末は等幅必須。UI フォント（IBM Plex Sans JP）を継承すると 1 文字ずつ間延びして
            // 崩れるので、コードフォント（既定はエディタと同じ・設定で変えられる・O25）を明示する。
            // 要素は text_style().font() を読むのでコンテナで指定すれば伝播する。
            .font_family(self.appearance.font_family.clone())
            .child(element::TerminalElement {
                terminal: cx.entity(),
            })
            .children(search_bar)
            .children(context_menu)
    }
}

/// 貼り付けるバイト列。bracketed paste の時は囲み、囲みを抜け出せないよう ESC を落とす。
/// 囲みが無い時は改行を CR にそろえる（Enter と同じ＝行ごとに実行される）。
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(text.replace('\x1b', "").as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
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

/// パレットの 0〜15（取り込んだ配色があればそちら）。
fn palette_hsla(index: usize, colors: &TerminalColors) -> Hsla {
    let (red, green, blue) = colors.palette[index].unwrap_or(ANSI16[index]);
    rgb_hsla(red, green, blue)
}

/// 既定の文字色（取り込んだ配色があればそちら・無ければテーマ）。
fn foreground_hsla(theme: &Theme, colors: &TerminalColors) -> Hsla {
    colors
        .foreground
        .map(|(red, green, blue)| rgb_hsla(red, green, blue))
        .unwrap_or(theme.fg0)
}

/// 面の色（取り込んだ配色があればそちら・無ければテーマ）。
pub(crate) fn background_hsla(theme: &Theme, colors: &TerminalColors) -> Hsla {
    colors
        .background
        .map(|(red, green, blue)| rgb_hsla(red, green, blue))
        .unwrap_or(theme.bg1)
}

fn named_to_hsla(named: NamedColor, theme: &Theme, colors: &TerminalColors) -> Hsla {
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
        NamedColor::Foreground => return foreground_hsla(theme, colors),
        NamedColor::Background => return background_hsla(theme, colors),
        NamedColor::Cursor => return theme.fg0,
        _ => return foreground_hsla(theme, colors),
    };
    palette_hsla(index, colors)
}

fn indexed_to_hsla(index: u8, colors: &TerminalColors) -> Hsla {
    match index {
        0..=15 => palette_hsla(index as usize, colors),
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

fn ansi_to_hsla(color: AnsiColor, theme: &Theme, colors: &TerminalColors) -> Hsla {
    match color {
        AnsiColor::Named(named) => named_to_hsla(named, theme, colors),
        AnsiColor::Spec(rgb) => rgb_hsla(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(index) => indexed_to_hsla(index, colors),
    }
}

fn is_default_background(color: AnsiColor) -> bool {
    matches!(color, AnsiColor::Named(NamedColor::Background))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn a_custom_name_wins_over_the_shell_title(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update(cx, |terminal, cx| {
            terminal.set_title(Some("vim".to_string()), cx);
            assert_eq!(terminal.display_title(), Some("vim"));
            terminal.set_custom_title(Some("editor".to_string()), cx);
            assert_eq!(terminal.display_title(), Some("editor"));
            assert_eq!(terminal.title(), Some("vim"), "シェルのタイトルは残る");
            terminal.set_custom_title(Some("   ".to_string()), cx);
            assert_eq!(terminal.display_title(), Some("vim"), "空は外す");
        });
    }

    #[test]
    fn an_imported_scheme_replaces_the_palette_and_the_surface() {
        let theme = Theme::dark();
        let defaults = TerminalColors::default();
        assert_eq!(
            ansi_to_hsla(AnsiColor::Named(NamedColor::Background), &theme, &defaults),
            theme.bg1,
            "既定はアプリのテーマ"
        );
        assert_eq!(
            ansi_to_hsla(AnsiColor::Named(NamedColor::Red), &theme, &defaults),
            rgb_hsla(205, 49, 49)
        );
        let mut colors = TerminalColors::default();
        colors.palette[1] = Some((0xcc, 0x66, 0x66));
        colors.background = Some((0x1d, 0x1f, 0x21));
        colors.foreground = Some((0xc5, 0xc8, 0xc6));
        assert_eq!(
            ansi_to_hsla(AnsiColor::Named(NamedColor::Red), &theme, &colors),
            rgb_hsla(0xcc, 0x66, 0x66)
        );
        assert_eq!(
            ansi_to_hsla(AnsiColor::Indexed(1), &theme, &colors),
            rgb_hsla(0xcc, 0x66, 0x66),
            "256 色の 0〜15 も同じ"
        );
        assert_eq!(
            ansi_to_hsla(AnsiColor::Named(NamedColor::Green), &theme, &colors),
            rgb_hsla(13, 188, 121),
            "書いていない色は既定"
        );
        assert_eq!(background_hsla(&theme, &colors), rgb_hsla(0x1d, 0x1f, 0x21));
        assert_eq!(
            ansi_to_hsla(AnsiColor::Named(NamedColor::Foreground), &theme, &colors),
            rgb_hsla(0xc5, 0xc8, 0xc6)
        );
    }

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
                underline_color: None,
                hyperlink: None,
            });
            column += 1;
            if wide {
                cells.push(RenderCell {
                    point: AlacPoint::new(alacritty_terminal::index::Line(line), Column(column)),
                    character: ' ',
                    fg: AnsiColor::Named(NamedColor::Foreground),
                    bg: AnsiColor::Named(NamedColor::Background),
                    flags: Flags::WIDE_CHAR_SPACER,
                    underline_color: None,
                    hyperlink: None,
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
        let links = detect_links(&cells, &[]);
        assert_eq!(
            links,
            vec![TerminalLink {
                line: 0,
                columns: 6..22,
                target: TerminalLinkTarget::Path {
                    path: "src/main.rs".into(),
                    line: 10,
                },
            }]
        );

        // 全角が先にある行でも列がずれない（byte 範囲 → char 添字 → 列の変換）。
        let cells = row_cells(1, "エラー lib.rs:42");
        let links = detect_links(&cells, &[]);
        assert_eq!(links.len(), 1);
        // 「エラー」= 3 文字 × 2 列 + 空白 1 列 = 7 列目から。
        assert_eq!(links[0].columns, 7..16);
        assert_eq!(
            links[0].target,
            TerminalLinkTarget::Path {
                path: "lib.rs".into(),
                line: 42,
            }
        );
    }

    #[test]
    fn detect_links_ignores_paths_without_line_numbers() {
        // ターミナルは実在確認をしないので、行番号の無いトークンはリンクにしない
        // （`ls` の出力を全部下線にしない）。
        assert!(detect_links(&row_cells(0, "Cargo.toml  README.md"), &[]).is_empty());
    }

    #[test]
    fn urls_become_links_including_local_dev_servers() {
        let cells = row_cells(0, "ready: http://localhost:5173/ and 127.0.0.1:8080");
        let links = detect_links(&cells, &[]);
        let targets: Vec<_> = links.iter().map(|link| link.target.clone()).collect();
        assert_eq!(
            targets,
            vec![
                TerminalLinkTarget::Url("http://localhost:5173/".into()),
                TerminalLinkTarget::Url("http://127.0.0.1:8080".into()),
            ]
        );
        assert_eq!(links[0].columns, 7..29);
    }

    #[test]
    fn osc8_hyperlinks_win_over_text_detection() {
        // `docs http://x.test` の全体が OSC 8 で https://example.com を指している。
        let mut cells = row_cells(0, "see docs http://x.test end");
        for cell in &mut cells[4..18] {
            cell.hyperlink = Some(0);
        }
        let hyperlinks = vec!["https://example.com/docs".to_string()];
        let links = detect_links(&cells, &hyperlinks);
        assert_eq!(
            links,
            vec![TerminalLink {
                line: 0,
                columns: 4..18,
                target: TerminalLinkTarget::Url("https://example.com/docs".into()),
            }],
            "文字から拾う URL は OSC 8 と重なるので出さない"
        );
        // http / https 以外の OSC 8 は開かない（`file:` や独自スキームを勝手に開かない）。
        let hyperlinks = vec!["vscode://open?x".to_string()];
        let links = detect_links(&cells, &hyperlinks);
        assert!(links
            .iter()
            .all(|link| link.target == TerminalLinkTarget::Url("http://x.test".into())));
    }

    #[test]
    fn wheel_lines_accumulates_fractions() {
        // 1 行未満は持ち越し、合計が 1 行に達した時だけ行が進む（トラックパッドの細かい増分）。
        let line_height = TerminalAppearance::default().line_height();
        let (lines, carry) = wheel_lines(0.0, line_height * 0.6, line_height);
        assert_eq!(lines, 0);
        let (lines, carry) = wheel_lines(carry, line_height * 0.6, line_height);
        assert_eq!(lines, 1);
        assert!(carry.abs() < 1.0);
        // 下方向（負の増分）も対称に畳まれる。
        let (lines, _) = wheel_lines(0.0, -line_height * 2.5, line_height);
        assert_eq!(lines, -2);
    }

    #[test]
    fn screen_of_reads_visible_lines_and_recent_scrollback() {
        let (events_tx, _events_rx) = unbounded::<AlacEvent>();
        let size = TerminalSize {
            columns: 20,
            lines: 3,
        };
        let config = Config {
            scrolling_history: 100,
            ..Config::default()
        };
        let mut term = Term::new(config, &size, Listener(events_tx));
        let mut parser = alacritty_terminal::vte::ansi::Processor::<
            alacritty_terminal::vte::ansi::StdSyncHandler,
        >::new();
        parser.advance(&mut term, "one\r\ntwo\r\n猫 three\r\nfour\r\n$ ".as_bytes());
        let screen = screen_of(&term, 2, false);
        // 3 行の画面に最後の 3 行、その上に scrollback が古い順で 2 行（要求した分だけ）。
        assert_eq!(screen.visible, vec!["猫 three", "four", "$"]);
        assert_eq!(screen.scrollback, vec!["one", "two"]);
        assert_eq!((screen.cursor_line, screen.cursor_column), (2, 2));
        // 履歴より多く求めても、ある分だけ。
        assert_eq!(screen_of(&term, 50, true).scrollback.len(), 2);
        // ユーザーが遡って見ていても、読むのは今の画面。
        term.scroll_display(Scroll::Delta(2));
        assert_eq!(
            screen_of(&term, 0, false).visible,
            vec!["猫 three", "four", "$"]
        );
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
mod action_tests {
    use super::*;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    /// PTY の出力と同じ経路（vte の解析器）で端末へ文字を流す。
    fn feed(terminal: &TerminalView, text: &str) {
        let mut parser = Processor::<StdSyncHandler>::new();
        parser.advance(&mut *terminal.term.lock(), text.as_bytes());
    }

    fn line_text(terminal: &TerminalView, line: i32) -> String {
        let term = terminal.term.lock();
        let columns = term.columns();
        (0..columns)
            .map(|column| term.grid()[Line(line)][Column(column)].c)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn dropped_paths_are_quoted_for_the_shell() {
        assert_eq!(quote_path_for_shell("/tmp/a.txt", false), "/tmp/a.txt");
        assert_eq!(
            quote_path_for_shell("/Users/me/Application Support/x", false),
            "'/Users/me/Application Support/x'"
        );
        assert_eq!(
            quote_path_for_shell("/tmp/it's here", false),
            "'/tmp/it'\\''s here'"
        );
        assert_eq!(quote_path_for_shell("/tmp/$HOME", false), "'/tmp/$HOME'");
        assert_eq!(
            quote_path_for_shell(r"C:\Users\me\a.txt", true),
            r"C:\Users\me\a.txt"
        );
        assert_eq!(
            quote_path_for_shell(r"C:\Program Files\x", true),
            r#""C:\Program Files\x""#
        );
    }

    /// O25: 見た目の設定を変えると、開いている端末がその場で従う（scrollback を減らせば古い行を
    /// 捨て、カーソルの既定が替わる。アプリが DECSCUSR で指定した形はそちらが勝つ）。
    #[gpui::test]
    fn open_terminals_follow_the_appearance_setting(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, _window, _cx| {
            assert_eq!(terminal.debug_appearance(), &TerminalAppearance::default());
            let output: String = (0..300).map(|index| format!("out {index}\r\n")).collect();
            feed(terminal, &output);
            assert!(terminal.term.lock().grid().history_size() > 100);
            assert_eq!(
                terminal.term.lock().cursor_style().shape,
                CursorShape::Block
            );
        });
        cx.update(|_window, cx| {
            cx.set_global(TerminalAppearance::new(16.0, "Menlo", 100, "bar"));
        });
        cx.run_until_parked();
        terminal.update_in(cx, |terminal, _window, _cx| {
            let appearance = terminal.debug_appearance();
            assert_eq!(appearance.font_size, 16.0);
            assert_eq!(appearance.font_family.as_ref(), "Menlo");
            let term = terminal.term.lock();
            assert!(
                term.grid().history_size() <= 100,
                "減らした分は古い方から捨てる"
            );
            assert_eq!(term.cursor_style().shape, CursorShape::Beam);
        });
        terminal.update_in(cx, |terminal, _window, _cx| {
            // アプリの指定（DECSCUSR 4 = 下線）は既定より勝つ。
            feed(terminal, "\x1b[4 q");
            assert_eq!(
                terminal.term.lock().cursor_style().shape,
                CursorShape::Underline
            );
        });
    }

    #[gpui::test]
    fn dropped_files_are_pasted_as_quoted_paths(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, window, cx| {
            feed(terminal, "\x1b[?2004h");
            let paths = [PathBuf::from("/tmp/a b.png"), PathBuf::from("/tmp/c.txt")];
            terminal.drop_paths(&paths, window, cx);
            // 引用の仕方はシェルに合わせる（unix の sh は '…'・Windows の PowerShell / cmd は "…"）。
            let expected: &[u8] = if cfg!(windows) {
                b"\x1b[200~\"/tmp/a b.png\" /tmp/c.txt \x1b[201~"
            } else {
                b"\x1b[200~'/tmp/a b.png' /tmp/c.txt \x1b[201~"
            };
            assert_eq!(terminal.debug_written_input(), expected);
        });
    }

    #[test]
    fn pasted_text_cannot_escape_bracketed_paste() {
        assert_eq!(
            paste_bytes("ls\x1b[201~rm -rf /\n", true),
            b"\x1b[200~ls[201~rm -rf /\n\x1b[201~"
        );
        // 囲みが無い時は改行を CR にそろえる（Enter と同じ）。
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[gpui::test]
    fn clear_keeps_the_prompt_line_and_drops_scrollback(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, window, cx| {
            let output: String = (0..300).map(|index| format!("out {index}\r\n")).collect();
            feed(terminal, &format!("{output}$ prompt"));
            assert!(terminal.term.lock().grid().history_size() > 0);
            terminal.clear(&actions::Clear, window, cx);
            let term = terminal.term.lock();
            assert_eq!(term.grid().history_size(), 0, "scrollback は消える");
            assert_eq!(term.grid().cursor.point.line, Line(0));
            drop(term);
            assert_eq!(line_text(terminal, 0), "$ prompt", "いまの行は先頭に残る");
            assert_eq!(line_text(terminal, 1), "");
        });
    }

    /// アプリがマウスの報告（1000 + SGR 1006）を有効にしたら、押す・離すを報告し、選択はしない。
    /// ⇧ を押している間は従来どおり選択。
    #[gpui::test]
    fn mouse_reports_go_to_the_app_unless_shift_is_held(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, _window, cx| {
            feed(terminal, "\x1b[?1000h\x1b[?1006h");
            terminal.sync(cx);
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let frame = terminal
            .read_with(cx, |terminal, _| terminal.grid_frame)
            .expect("描画でグリッドの座標が決まる");
        // 3 列目・2 行目のセルの中央。
        let position = frame.origin + point(frame.cell_width * 2.5, frame.line_height * 1.5);
        cx.simulate_mouse_down(position, MouseButton::Left, gpui::Modifiers::none());
        cx.simulate_mouse_up(position, MouseButton::Left, gpui::Modifiers::none());
        terminal.read_with(cx, |terminal, _| {
            assert_eq!(terminal.debug_written_input(), b"\x1b[<0;3;2M\x1b[<0;3;2m");
            assert!(
                terminal.term.lock().selection.is_none(),
                "報告中は選択しない"
            );
        });
        let shift = gpui::Modifiers {
            shift: true,
            ..gpui::Modifiers::none()
        };
        cx.simulate_mouse_down(position, MouseButton::Left, shift);
        cx.simulate_mouse_move(
            position + point(frame.cell_width * 3., px(0.)),
            Some(MouseButton::Left),
            shift,
        );
        terminal.read_with(cx, |terminal, _| {
            assert_eq!(
                terminal.debug_written_input().len(),
                b"\x1b[<0;3;2M\x1b[<0;3;2m".len(),
                "⇧ の間は報告しない"
            );
            assert!(terminal.term.lock().selection.is_some(), "⇧ の間は選択する");
        });
    }

    /// FOCUS_IN_OUT（1004）を有効にしたアプリへ、フォーカスの出入りを `CSI I` / `CSI O` で伝える。
    #[gpui::test]
    fn focus_changes_are_reported_when_the_app_asks(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, _window, cx| {
            feed(terminal, "\x1b[?1004h");
            terminal.sync(cx);
        });
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        // フォーカスの出入りの通知は次の描画で配られる。
        terminal.update_in(cx, |terminal, window, cx| {
            window.focus(&terminal.focus_handle, cx);
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        terminal.update_in(cx, |_terminal, window, _cx| window.blur());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        terminal.read_with(cx, |terminal, _| {
            assert_eq!(terminal.debug_written_input(), b"\x1b[I\x1b[O");
        });
    }

    /// OSC 52 の書き込みはクリップボードへ、読み出しには応えない。タイトルはタブの名前に使う。
    /// 色の問い合わせには描画と同じ色で答える。
    #[gpui::test]
    fn terminal_requests_from_apps(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, _window, cx| {
            terminal.on_alac_event(
                AlacEvent::ClipboardStore(ClipboardType::Clipboard, "copied".into()),
                cx,
            );
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("copied".to_string())
            );
            terminal.on_alac_event(
                AlacEvent::ClipboardLoad(
                    ClipboardType::Clipboard,
                    Arc::new(|text| format!("\x1b]52;c;{text}\x07")),
                ),
                cx,
            );
            assert!(
                terminal.debug_written_input().is_empty(),
                "読み出しには応えない"
            );

            terminal.on_alac_event(AlacEvent::Title("✳ Claude Code\n\x07x".into()), cx);
            assert_eq!(terminal.title(), Some("✳ Claude Code"));
            terminal.on_alac_event(AlacEvent::ResetTitle, cx);
            assert_eq!(terminal.title(), None);

            // 257 = 背景（dark の bg1 = #1b1e25）。
            terminal.on_alac_event(
                AlacEvent::ColorRequest(
                    257,
                    Arc::new(|rgb| format!("{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)),
                ),
                cx,
            );
            assert_eq!(terminal.debug_written_input(), b"1b1e25");
        });
    }

    #[gpui::test]
    fn select_all_covers_the_scrollback(cx: &mut gpui::TestAppContext) {
        let (terminal, cx) = cx.add_window_view(|_, cx| TerminalView::new_test(Theme::dark(), cx));
        terminal.update_in(cx, |terminal, window, cx| {
            let output: String = (0..30).map(|index| format!("row {index}\r\n")).collect();
            feed(terminal, &output);
            terminal.select_all(&actions::SelectAll, window, cx);
            let text = terminal
                .term
                .lock()
                .selection_to_string()
                .unwrap_or_default();
            assert!(text.starts_with("row 0\n"));
            assert!(text.contains("row 29"));
        });
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
            assert_eq!(
                terminal
                    .selected_text_range(false, window, cx)
                    .unwrap()
                    .range,
                1..3
            );
            assert_eq!(terminal.text_for_range(1..3, &mut None, window, cx).as_deref(), Some("🐱"));
            terminal.unmark_text(window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), None);
            assert!(terminal.written_input.borrow().is_empty());
            terminal.replace_and_mark_text_in_range(None, "", None, window, cx);
            assert_eq!(terminal.marked_text_range(window, cx), None);
        });
    }
}
