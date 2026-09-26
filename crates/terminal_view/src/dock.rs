use crate::{TerminalEvent, TerminalView};
use gpui::{
    div, prelude::*, px, App, Context, Entity, EventEmitter, Hsla, IntoElement, MouseButton,
    PromptButton, PromptLevel, Render, SharedString, StyleRefinement, Window,
};
use std::path::PathBuf;
use theme_core::Theme;
use ui::Tooltip;

/// 端末を作る場所と shell。ProjectSession が Host から一度だけ解決して渡す。
#[derive(Clone, Default)]
pub struct TerminalLaunch {
    pub cwd: Option<PathBuf>,
    pub shell: Option<(String, Vec<String>)>,
}

/// TerminalDock から shell への通知。
pub enum TerminalDockEvent {
    OpenPath {
        path: String,
        line: u32,
    },
    /// 端末の URL のクリック（`TerminalEvent::OpenUrl` をそのまま上げる）。
    OpenUrl(String),
    Dismissed,
}

/// 1 project に属する端末タブ群。PTY と active index のライフサイクルをまとめて所有する。
pub struct TerminalDock {
    terminals: Vec<Entity<TerminalView>>,
    /// Fleet に配置した端末。ドックの表示リストから外しても同じ PTY を保持する。
    detached: std::collections::BTreeMap<u64, Entity<TerminalView>>,
    active: usize,
    launch: TerminalLaunch,
    theme: Theme,
    /// プロジェクト色（各端末の検索欄の枠・キャレット）。
    accent: Hsla,
    /// テストで PTY を起動しない（[`Self::use_test_terminals`]）。本番のビルドには存在しない。
    #[cfg(feature = "test-support")]
    test_terminals: bool,
}

impl TerminalDock {
    pub fn new(launch: TerminalLaunch, theme: Theme) -> Self {
        Self {
            terminals: Vec::new(),
            detached: Default::default(),
            active: 0,
            launch,
            accent: theme.fg2,
            theme,
            #[cfg(feature = "test-support")]
            test_terminals: false,
        }
    }

    /// 以後この Dock が作る端末を PTY 無しにする（`gpui::test` の決定性のため）。
    /// `start_session` 経由で端末が増える経路（Fleet の「端末を足す」）をテストから
    /// 通せるようにするための口。本番の生成経路には入らない。
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn use_test_terminals(&mut self) {
        self.test_terminals = true;
    }

    /// 以後作る端末と、今ある端末のプロジェクト色。
    pub fn with_accent(mut self, accent: Hsla) -> Self {
        self.accent = accent;
        self
    }

    /// プロジェクト色が変わった時（レールの色の変更）。今ある端末にも配る。
    pub fn set_accent(&mut self, accent: Hsla, cx: &mut Context<Self>) {
        self.accent = accent;
        for terminal in self.terminals.iter().chain(self.detached.values()) {
            terminal.update(cx, |terminal, cx| terminal.set_accent(accent, cx));
        }
    }

    fn create_terminal(
        &self,
        launch: TerminalLaunch,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let theme = self.theme.clone();
        let accent = self.accent;
        #[cfg(feature = "test-support")]
        if self.test_terminals {
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new_test(theme, cx);
                terminal.accent = accent;
                terminal
            });
            cx.subscribe(&terminal, Self::on_terminal_event).detach();
            return terminal;
        }
        let terminal = cx.new(|cx| {
            let mut terminal = TerminalView::new_with_shell(launch.cwd, launch.shell, theme, cx);
            terminal.accent = accent;
            terminal
        });
        cx.subscribe(&terminal, Self::on_terminal_event).detach();
        terminal
    }

    fn on_terminal_event(
        &mut self,
        _: Entity<TerminalView>,
        event: &TerminalEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalEvent::OpenPath { path, line } => {
                cx.emit(TerminalDockEvent::OpenPath {
                    path: path.clone(),
                    line: *line,
                });
            }
            TerminalEvent::OpenUrl(url) => cx.emit(TerminalDockEvent::OpenUrl(url.clone())),
            // タブの名前（アプリのタイトル）を描き直す。
            TerminalEvent::TitleChanged => cx.notify(),
        }
    }

    /// アクティブ端末（未生成なら None）。プロジェクト切替のフォーカス追従が使う読み取り専用アクセサ
    /// （[`Self::ensure_active`] と違い PTY を起動しない）。
    pub fn active_terminal(&self) -> Option<Entity<TerminalView>> {
        self.terminals.get(self.active).cloned()
    }

    pub fn session(&self, id: u64) -> Option<Entity<TerminalView>> {
        self.detached.get(&id).cloned()
    }

    /// 下ドックのタブの端末（左から）と、その中のアクティブの位置（CLI の `terminal list`）。
    /// 読み取りだけ＝PTY を起動しない。
    pub fn tab_terminals(&self) -> (&[Entity<TerminalView>], usize) {
        (&self.terminals, self.active)
    }

    /// Fleet の Task カードに置いた端末（id 順・CLI の `terminal list`）。
    pub fn placed_terminals(&self) -> Vec<(u64, Entity<TerminalView>)> {
        self.detached
            .iter()
            .map(|(id, terminal)| (*id, terminal.clone()))
            .collect()
    }

    pub fn detached_sessions(&self) -> Vec<u64> {
        self.detached.keys().copied().collect()
    }

    /// 明示的な起動操作だけで呼ぶ。保存配置の復元や Render は shell を起動しない。
    pub fn start_session(&mut self, id: u64, cx: &mut Context<Self>) -> Entity<TerminalView> {
        if let Some(terminal) = self.detached.get(&id) {
            return terminal.clone();
        }
        let terminal = self.create_terminal(self.launch.clone(), cx);
        self.detached.insert(id, terminal.clone());
        cx.notify();
        terminal
    }

    pub fn detach_active(&mut self, id: u64, cx: &mut Context<Self>) -> Entity<TerminalView> {
        self.ensure_active(cx);
        let terminal = self.terminals.remove(self.active);
        self.active = self.active.min(self.terminals.len().saturating_sub(1));
        self.detached.insert(id, terminal.clone());
        cx.notify();
        terminal
    }

    pub fn attach_session(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self.detached.remove(&id) {
            self.terminals.push(terminal);
            self.active = self.terminals.len() - 1;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub fn terminate_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.detached.remove(&id);
        cx.notify();
    }

    /// 前面でプロセスが動いている端末の数（ドックのタブと Fleet に置いた端末の両方）。
    /// ⌘Q・窓を閉じる時の確認（O4）が数える。
    pub fn busy_terminal_count(&self, cx: &App) -> usize {
        self.terminals
            .iter()
            .chain(self.detached.values())
            .filter(|terminal| terminal.read(cx).has_foreground_process())
            .count()
    }

    /// dock 内のいずれかの端末にキーボードフォーカスがあるか（プロジェクト切替のフォーカス追従判定）。
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.terminals
            .iter()
            .chain(self.detached.values())
            .any(|terminal| {
                terminal
                    .read(cx)
                    .focus_handle()
                    .contains_focused(window, cx)
            })
    }

    pub fn ensure_active(&mut self, cx: &mut Context<Self>) -> Entity<TerminalView> {
        if self.terminals.is_empty() {
            let terminal = self.create_terminal(self.launch.clone(), cx);
            self.terminals.push(terminal);
            self.active = 0;
        }
        self.active = self.active.min(self.terminals.len() - 1);
        self.terminals[self.active].clone()
    }

    /// PTY を起動せず、実際の terminal tab vector と active index を使うテスト用経路。
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn ensure_active_test(&mut self, cx: &mut Context<Self>) -> Entity<TerminalView> {
        if self.terminals.is_empty() {
            let theme = self.theme.clone();
            let accent = self.accent;
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new_test(theme, cx);
                terminal.accent = accent;
                terminal
            });
            cx.subscribe(&terminal, Self::on_terminal_event).detach();
            self.terminals.push(terminal);
            self.active = 0;
        }
        self.active = self.active.min(self.terminals.len() - 1);
        self.terminals[self.active].clone()
    }

    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.ensure_active(cx);
        window.focus(&terminal.read(cx).focus_handle(), cx);
    }

    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.create_terminal(self.launch.clone(), cx);
        self.terminals.push(terminal);
        self.active = self.terminals.len() - 1;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn open_command(
        &mut self,
        launch: TerminalLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal = self.create_terminal(launch, cx);
        self.terminals.push(terminal);
        self.active = self.terminals.len() - 1;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn is_any_focused(&self, window: &Window, cx: &App) -> bool {
        self.terminals
            .iter()
            .any(|terminal| terminal.read(cx).focus_handle().is_focused(window))
    }

    pub fn focus_active_if_present(&self, window: &mut Window, cx: &mut App) {
        if let Some(terminal) = self.terminals.get(self.active) {
            window.focus(&terminal.read(cx).focus_handle(), cx);
        }
    }

    pub fn insert_text(&self, text: &str, window: &mut Window, cx: &mut App) {
        if let Some(terminal) = self.terminals.get(self.active) {
            terminal.read(cx).insert_text(text);
            window.focus(&terminal.read(cx).focus_handle(), cx);
        }
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        for terminal in self.terminals.iter().chain(self.detached.values()) {
            terminal.update(cx, |terminal, cx| terminal.set_theme(theme.clone(), cx));
        }
        cx.notify();
    }

    /// ProjectSession 作成前の互換経路。session 化後は dock 自体を切り替えるため不要になる。
    pub fn reset_launch(&mut self, launch: TerminalLaunch, cx: &mut Context<Self>) {
        self.terminals.clear();
        self.detached.clear();
        self.active = 0;
        self.launch = launch;
        cx.notify();
    }

    pub fn emit_open_path(&mut self, path: String, line: u32, cx: &mut Context<Self>) {
        cx.emit(TerminalDockEvent::OpenPath { path, line });
    }

    fn switch_to(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.terminals.len() {
            self.active = index;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// タブの ×。前面でプロセスが動いている端末は、閉じる（= SIGHUP で止まる）前に確認する（O4）。
    /// 確認は OS のダイアログ（`Window::prompt`・mac はシート）で、⏎ = 閉じる / Esc = キャンセル。
    /// 答えを待つ間にタブの並びが変わってもよいよう、閉じる対象は添字でなく Entity で覚える。
    fn close(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminals.get(index).cloned() else {
            return;
        };
        if !terminal.read(cx).has_foreground_process() {
            self.close_now(index, window, cx);
            return;
        }
        let message = i18n::t!("terminal.close_busy_title", "n" => index + 1);
        let detail = i18n::t!("terminal.close_busy_detail");
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(&detail),
            &[
                PromptButton::ok(i18n::t!("terminal.close_busy_confirm")),
                PromptButton::cancel(i18n::t!("terminal.close_busy_cancel")),
            ],
            cx,
        );
        let target = terminal.entity_id();
        cx.spawn_in(window, async move |dock, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let closed = dock.update_in(cx, |dock, window, cx| {
                if let Some(index) = dock
                    .terminals
                    .iter()
                    .position(|terminal| terminal.entity_id() == target)
                {
                    dock.close_now(index, window, cx);
                }
            });
            if let Err(error) = closed {
                eprintln!("ターミナルを閉じられない（ドックが既に無い）: {error:#}");
            }
        })
        .detach();
    }

    fn close_now(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.terminals.len() {
            return;
        }
        self.terminals.remove(index);
        if self.terminals.is_empty() {
            self.active = 0;
            cx.emit(TerminalDockEvent::Dismissed);
        } else {
            if index <= self.active && self.active > 0 {
                self.active -= 1;
            }
            self.active = self.active.min(self.terminals.len() - 1);
            self.focus_active(window, cx);
        }
        cx.notify();
    }
}

impl EventEmitter<TerminalDockEvent> for TerminalDock {}

impl Render for TerminalDock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.active;
        let mut header = div()
            .flex()
            .items_center()
            .h(px(28.))
            .flex_none()
            .bg(theme.bg0)
            .border_t_1()
            .border_b_1()
            .border_color(theme.border);
        for index in 0..self.terminals.len() {
            let is_active = index == active;
            header = header.child(
                div()
                    .id(("term-tab", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h_full()
                    .px(px(10.))
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .text_color(if is_active { theme.fg0 } else { theme.fg2 })
                    .when(is_active, |element| element.bg(theme.bg1))
                    .hover(|style| style.bg(theme.bg1))
                    .child(
                        // アプリが付けたタイトル（OSC 0 / 2・`✳ Claude Code` など）。無ければ連番。
                        div()
                            .max_w(px(220.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(
                                self.terminals[index]
                                    .read(cx)
                                    .title()
                                    .map(SharedString::from)
                                    .unwrap_or_else(|| {
                                        SharedString::from(i18n::t!(
                                            "terminal.tab_title",
                                            "n" => index + 1
                                        ))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id(("term-tab-close", index))
                            .flex_none()
                            .text_color(theme.fg2)
                            .hover(|style| style.text_color(theme.fg0))
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close(index, window, cx);
                                }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.switch_to(index, window, cx)),
                    ),
            );
        }
        let header = header
            .child(
                div()
                    .id("term-add")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(28.))
                    .h_full()
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg1))
                    .child("＋")
                    .tooltip(Tooltip::text(i18n::t!("terminal.add_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.add(window, cx)),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("term-close-dock")
                    .px(px(8.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0))
                    .child("×")
                    .tooltip(Tooltip::text(
                        i18n::t!("terminal.close_dock_tip"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _, _window, cx| cx.emit(TerminalDockEvent::Dismissed)),
                    ),
            );
        let body = match self.terminals.get(active) {
            Some(terminal) => div().flex_1().min_h_0().overflow_hidden().child(
                terminal
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
            None => div().flex_1(),
        };
        // 高さは**置いた側**（workspace の下段ドック / 編隊セル）が決める。ここで固定高を持つと
        // 高さドラッグも、セルいっぱいに広がることもできない（2026-07-27 に固定 240px を撤去）。
        //
        // **`size_full()` であって `flex_1()` ではない**（2026-08-23・Windows 実機で発覚）。
        // この view は置いた側が `.cached(...)` で差し込む。gpui の cached は
        // **children を `None` にして layout を要求する**（"caching skips rendering the contents
        // to measure them" — `gpui/src/view.rs`）ため、**cached の subtree は隔離してレイアウト**される。
        // 隔離された subtree の root で `flex_1()` は効かない（flex 親が居ない）＝高さ 0 に潰れ、
        // 端末のグリッドが 24 セルまで縮んで**中身が真っ黒**に見えた。
        // `size_full()` なら「与えられた領域いっぱい」になるので、置いた側の意図（上記）も保たれる。
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .child(header)
            .child(body)
    }
}
