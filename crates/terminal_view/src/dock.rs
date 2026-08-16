use crate::{TerminalEvent, TerminalView};
use gpui::{
    div, prelude::*, px, App, Context, Entity, EventEmitter, IntoElement, MouseButton, Render,
    StyleRefinement, Window,
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
    OpenPath { path: String, line: u32 },
    Dismissed,
}

/// 1 project に属する端末タブ群。PTY と active index のライフサイクルをまとめて所有する。
pub struct TerminalDock {
    terminals: Vec<Entity<TerminalView>>,
    active: usize,
    launch: TerminalLaunch,
    theme: Theme,
}

impl TerminalDock {
    pub fn new(launch: TerminalLaunch, theme: Theme) -> Self {
        Self {
            terminals: Vec::new(),
            active: 0,
            launch,
            theme,
        }
    }

    fn create_terminal(
        &self,
        launch: TerminalLaunch,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let theme = self.theme.clone();
        let terminal =
            cx.new(|cx| TerminalView::new_with_shell(launch.cwd, launch.shell, theme, cx));
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
        }
    }

    /// アクティブ端末（未生成なら None）。プロジェクト切替のフォーカス追従が使う読み取り専用アクセサ
    /// （[`Self::ensure_active`] と違い PTY を起動しない）。
    pub fn active_terminal(&self) -> Option<Entity<TerminalView>> {
        self.terminals.get(self.active).cloned()
    }

    /// dock 内のいずれかの端末にキーボードフォーカスがあるか（プロジェクト切替のフォーカス追従判定）。
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.terminals.iter().any(|terminal| {
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
            let terminal = cx.new(|cx| TerminalView::new_test(theme, cx));
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
        for terminal in &self.terminals {
            terminal.update(cx, |terminal, cx| terminal.set_theme(theme.clone(), cx));
        }
        cx.notify();
    }

    /// ProjectSession 作成前の互換経路。session 化後は dock 自体を切り替えるため不要になる。
    pub fn reset_launch(&mut self, launch: TerminalLaunch, cx: &mut Context<Self>) {
        self.terminals.clear();
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

    fn close(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
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
                    .child(i18n::t!("terminal.tab_title", "n" => index + 1))
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
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .child(header)
            .child(body)
    }
}
