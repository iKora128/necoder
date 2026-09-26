use crate::{TerminalEvent, TerminalView};
use gpui::{
    div, prelude::*, px, svg, App, Context, Entity, EventEmitter, Global, Hsla, IntoElement,
    MouseButton, PromptButton, PromptLevel, Render, SharedString, StyleRefinement, Window,
};
use std::path::PathBuf;
use theme_core::Theme;
use ui::Tooltip;

/// 端末を作る場所と shell。ProjectSession が Host から一度だけ解決して渡す。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalLaunch {
    pub cwd: Option<PathBuf>,
    pub shell: Option<(String, Vec<String>)>,
}

impl TerminalLaunch {
    /// 設定で選んだシェルを当てた launch（O25）。当てるのは**手元の端末**（cwd を持つ launch）だけ:
    /// SSH 先の端末は `ssh -tt …` 自体が launch で cwd を持たないので、そのまま（接続先のシェルに任せる）。
    /// シェルが空なら何もしない（OS の既定 = mac / Linux は `$SHELL`・Windows は pwsh → powershell）。
    pub fn with_shell(mut self, shell: Option<&TerminalShell>) -> Self {
        let Some(shell) = shell else {
            return self;
        };
        let program = shell.program.trim();
        if program.is_empty() || self.cwd.is_none() {
            return self;
        }
        self.shell = Some((program.to_string(), shell.args.clone()));
        self
    }
}

/// 設定で選んだシェルと引数（O25・`terminal_shell` / `terminal_shell_args`）。workspace が設定から作って
/// global に置く。新しく開く手元の端末にだけ効く（開いている端末は作り直さない・コマンドを走らせる
/// 端末は [`TerminalDock::open_command`] の指定のまま）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalShell {
    pub program: String,
    pub args: Vec<String>,
}

impl Global for TerminalShell {}

/// よく使うコマンド 1 つ（O25・Quick Commands）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickCommand {
    /// ボタンに出す名前。
    pub name: String,
    /// 新しい端末で走らせるコマンド（シェルに打つのと同じ）。
    pub command: String,
}

/// よく使うコマンドの一覧（`quick_commands`）。workspace が設定から作って global に置く。
/// ドックの ▶ から選ぶと、新しい端末を開いてそこで走らせる（SSH 先のプロジェクトなら SSH 先で）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuickCommands(pub Vec<QuickCommand>);

impl Global for QuickCommands {}

/// TerminalDock から shell への通知。
pub enum TerminalDockEvent {
    OpenPath {
        path: String,
        line: u32,
    },
    /// 端末の URL のクリック（`TerminalEvent::OpenUrl` をそのまま上げる）。
    OpenUrl(String),
    Dismissed,
    /// タブのダブルクリック（改名・O24）。入力欄は親が出す（このクレートはエディタを知らない）。
    RenameRequested(Entity<TerminalView>),
}

/// ドックのタブ 1 つ（O24・分割）: 横に並べた端末。1 つなら分割していない。
struct TerminalTab {
    panes: Vec<Entity<TerminalView>>,
    /// タブの中で前にいる端末（打鍵が行く・⌘W が閉じる・CLI の「いまの端末」）。
    focused: usize,
}

impl TerminalTab {
    fn single(terminal: Entity<TerminalView>) -> Self {
        Self {
            panes: vec![terminal],
            focused: 0,
        }
    }

    fn focused_pane(&self) -> &Entity<TerminalView> {
        &self.panes[self.focused.min(self.panes.len() - 1)]
    }
}

/// 1 つのタブに横に並べられる端末の数（1 つ 1 つが狭くなりすぎない所まで）。
pub const MAX_PANES: usize = 4;

/// 1 project に属する端末タブ群。PTY と active index のライフサイクルをまとめて所有する。
/// タブは横に分割できる（O24・⌘\・VSCode の「ターミナルの分割」と同じ 1 段）。
pub struct TerminalDock {
    tabs: Vec<TerminalTab>,
    /// Fleet に配置した端末。ドックの表示リストから外しても同じ PTY を保持する。
    detached: std::collections::BTreeMap<u64, Entity<TerminalView>>,
    active: usize,
    launch: TerminalLaunch,
    theme: Theme,
    /// プロジェクト色（各端末の検索欄の枠・キャレット）。
    accent: Hsla,
    /// よく使うコマンドの帯を開いているか（ドックの ▶・O25）。
    quick_open: bool,
    /// テストで PTY を起動しない（[`Self::use_test_terminals`]）。本番のビルドには存在しない。
    #[cfg(feature = "test-support")]
    test_terminals: bool,
}

impl TerminalDock {
    pub fn new(launch: TerminalLaunch, theme: Theme) -> Self {
        Self {
            tabs: Vec::new(),
            detached: Default::default(),
            active: 0,
            launch,
            quick_open: false,
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

    /// テスト用: タブの数と前にあるタブの番号。
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn debug_tabs(&self) -> (usize, usize) {
        (self.tabs.len(), self.active)
    }

    /// テスト用: 前にあるタブの端末の数と、その中で前にいる端末の番号。
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn debug_panes(&self) -> (usize, usize) {
        self.tabs
            .get(self.active)
            .map(|tab| (tab.panes.len(), tab.focused))
            .unwrap_or((0, 0))
    }

    /// ドックのタブの端末（分割も含めて左から）。
    fn docked(&self) -> impl Iterator<Item = &Entity<TerminalView>> {
        self.tabs.iter().flat_map(|tab| tab.panes.iter())
    }

    /// 以後作る端末と、今ある端末のプロジェクト色。
    pub fn with_accent(mut self, accent: Hsla) -> Self {
        self.accent = accent;
        self
    }

    /// プロジェクト色が変わった時（レールの色の変更）。今ある端末にも配る。
    pub fn set_accent(&mut self, accent: Hsla, cx: &mut Context<Self>) {
        self.accent = accent;
        let terminals: Vec<Entity<TerminalView>> = self
            .docked()
            .chain(self.detached.values())
            .cloned()
            .collect();
        for terminal in terminals {
            terminal.update(cx, |terminal, cx| terminal.set_accent(accent, cx));
        }
    }

    /// シェルを開く端末の launch（設定のシェルを当てる・O25）。
    fn shell_launch(&self, cx: &App) -> TerminalLaunch {
        self.launch
            .clone()
            .with_shell(cx.try_global::<TerminalShell>())
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
    /// （[`Self::ensure_active`] と違い PTY を起動しない）。分割したタブなら前にいる端末。
    pub fn active_terminal(&self) -> Option<Entity<TerminalView>> {
        self.tabs
            .get(self.active)
            .map(|tab| tab.focused_pane().clone())
    }

    pub fn session(&self, id: u64) -> Option<Entity<TerminalView>> {
        self.detached.get(&id).cloned()
    }

    /// 下ドックの端末（タブの順・分割は左から）と、その中のアクティブの位置（CLI の `terminal list`）。
    /// 読み取りだけ＝PTY を起動しない。
    pub fn tab_terminals(&self) -> (Vec<Entity<TerminalView>>, usize) {
        let terminals: Vec<Entity<TerminalView>> = self.docked().cloned().collect();
        let active = self
            .tabs
            .iter()
            .take(self.active)
            .map(|tab| tab.panes.len())
            .sum::<usize>()
            + self
                .tabs
                .get(self.active)
                .map(|tab| tab.focused.min(tab.panes.len() - 1))
                .unwrap_or(0);
        (terminals, active)
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
        let terminal = self.create_terminal(self.shell_launch(cx), cx);
        self.detached.insert(id, terminal.clone());
        cx.notify();
        terminal
    }

    /// 前にある端末を Fleet へ持ち出す。分割したタブならその端末だけ（残りはタブに残る）。
    pub fn detach_active(&mut self, id: u64, cx: &mut Context<Self>) -> Entity<TerminalView> {
        self.ensure_active(cx);
        let tab = &mut self.tabs[self.active];
        let pane = tab.focused.min(tab.panes.len() - 1);
        let terminal = tab.panes.remove(pane);
        if tab.panes.is_empty() {
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        } else {
            tab.focused = pane.min(tab.panes.len() - 1);
        }
        self.detached.insert(id, terminal.clone());
        cx.notify();
        terminal
    }

    pub fn attach_session(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self.detached.remove(&id) {
            self.tabs.push(TerminalTab::single(terminal));
            self.active = self.tabs.len() - 1;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub fn terminate_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.detached.remove(&id);
        cx.notify();
    }

    /// 前面でプロセスが動いている端末の数（ドックのタブと Fleet に置いた端末の両方）。
    /// ⌘Q・最後の窓を閉じる時の確認（O4）が数える。
    pub fn busy_terminal_count(&self, cx: &App) -> usize {
        self.docked()
            .chain(self.detached.values())
            .filter(|terminal| terminal.read(cx).has_foreground_process())
            .count()
    }

    /// dock 内のいずれかの端末にキーボードフォーカスがあるか（プロジェクト切替のフォーカス追従判定）。
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.docked().chain(self.detached.values()).any(|terminal| {
            terminal
                .read(cx)
                .focus_handle()
                .contains_focused(window, cx)
        })
    }

    pub fn ensure_active(&mut self, cx: &mut Context<Self>) -> Entity<TerminalView> {
        if self.tabs.is_empty() {
            let terminal = self.create_terminal(self.shell_launch(cx), cx);
            self.tabs.push(TerminalTab::single(terminal));
            self.active = 0;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self.tabs[self.active].focused_pane().clone()
    }

    /// PTY を起動せず、実際の terminal tab vector と active index を使うテスト用経路。
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn ensure_active_test(&mut self, cx: &mut Context<Self>) -> Entity<TerminalView> {
        if self.tabs.is_empty() {
            let theme = self.theme.clone();
            let accent = self.accent;
            let terminal = cx.new(|cx| {
                let mut terminal = TerminalView::new_test(theme, cx);
                terminal.accent = accent;
                terminal
            });
            cx.subscribe(&terminal, Self::on_terminal_event).detach();
            self.tabs.push(TerminalTab::single(terminal));
            self.active = 0;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self.tabs[self.active].focused_pane().clone()
    }

    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.ensure_active(cx);
        window.focus(&terminal.read(cx).focus_handle(), cx);
    }

    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.create_terminal(self.shell_launch(cx), cx);
        self.tabs.push(TerminalTab::single(terminal));
        self.active = self.tabs.len() - 1;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 前にあるタブを横に分割する（O24・C02）: 前にいる端末の右に新しい端末を開き、そこへ打鍵を移す。
    /// 1 つのタブは [`MAX_PANES`] まで（それ以上は新しいタブにする）。
    pub fn split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_active(cx);
        if self.tabs[self.active].panes.len() >= MAX_PANES {
            self.add(window, cx);
            return;
        }
        let terminal = self.create_terminal(self.shell_launch(cx), cx);
        let tab = &mut self.tabs[self.active];
        let position = tab.focused.min(tab.panes.len() - 1) + 1;
        tab.panes.insert(position, terminal);
        tab.focused = position;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 分割したタブの中で押した端末を前にする（打鍵は端末自身が受け取りに行く）。
    fn set_focused_pane(&mut self, tab: usize, pane: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(tab) {
            if pane < tab.panes.len() && tab.focused != pane {
                tab.focused = pane;
                cx.notify();
            }
        }
    }

    /// よく使うコマンドを新しい端末で走らせる（O25）。シェルを開いてからコマンドを打つので、終わっても
    /// 端末はシェルに戻って残る（出力を読める・続けて打てる）。
    pub fn run_quick_command(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = cx
            .try_global::<QuickCommands>()
            .and_then(|commands| commands.0.get(index).cloned())
        else {
            return;
        };
        self.quick_open = false;
        let terminal = self.create_terminal(self.shell_launch(cx), cx);
        terminal
            .read(cx)
            .send_input(&format!("{}\n", command.command));
        self.tabs.push(TerminalTab::single(terminal));
        self.active = self.tabs.len() - 1;
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
        self.tabs.push(TerminalTab::single(terminal));
        self.active = self.tabs.len() - 1;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn is_any_focused(&self, window: &Window, cx: &App) -> bool {
        self.docked()
            .any(|terminal| terminal.read(cx).focus_handle().is_focused(window))
    }

    pub fn focus_active_if_present(&self, window: &mut Window, cx: &mut App) {
        if let Some(tab) = self.tabs.get(self.active) {
            window.focus(&tab.focused_pane().read(cx).focus_handle(), cx);
        }
    }

    pub fn insert_text(&self, text: &str, window: &mut Window, cx: &mut App) {
        if let Some(tab) = self.tabs.get(self.active) {
            let terminal = tab.focused_pane();
            terminal.read(cx).insert_text(text);
            window.focus(&terminal.read(cx).focus_handle(), cx);
        }
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        let terminals: Vec<Entity<TerminalView>> = self
            .docked()
            .chain(self.detached.values())
            .cloned()
            .collect();
        for terminal in terminals {
            terminal.update(cx, |terminal, cx| terminal.set_theme(theme.clone(), cx));
        }
        cx.notify();
    }

    /// ProjectSession 作成前の互換経路。session 化後は dock 自体を切り替えるため不要になる。
    pub fn reset_launch(&mut self, launch: TerminalLaunch, cx: &mut Context<Self>) {
        self.tabs.clear();
        self.detached.clear();
        self.active = 0;
        self.launch = launch;
        cx.notify();
    }

    pub fn emit_open_path(&mut self, path: String, line: u32, cx: &mut Context<Self>) {
        cx.emit(TerminalDockEvent::OpenPath { path, line });
    }

    fn switch_to(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = index;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// 閉じる前に、前面でプロセスが動いている端末があれば確かめる（O4・閉じる = SIGHUP で止まる）。
    /// 確認は OS のダイアログ（`Window::prompt`・mac はシート）で、⏎ = 閉じる / Esc = キャンセル。
    /// 答えを待つ間に並びが変わってもよいよう、閉じる対象は添字でなく Entity で覚える。
    fn confirm_then(
        &mut self,
        terminals: Vec<Entity<TerminalView>>,
        tab_number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        close: fn(&mut Self, &[gpui::EntityId], &mut Window, &mut Context<Self>),
    ) {
        let targets: Vec<gpui::EntityId> = terminals
            .iter()
            .map(|terminal| terminal.entity_id())
            .collect();
        if !terminals
            .iter()
            .any(|terminal| terminal.read(cx).has_foreground_process())
        {
            close(self, &targets, window, cx);
            return;
        }
        let message = i18n::t!("terminal.close_busy_title", "n" => tab_number);
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
        cx.spawn_in(window, async move |dock, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let closed = dock.update_in(cx, |dock, window, cx| close(dock, &targets, window, cx));
            if let Err(error) = closed {
                eprintln!("ターミナルを閉じられない（ドックが既に無い）: {error:#}");
            }
        })
        .detach();
    }

    /// タブの ×: タブの端末を全部閉じる（分割も含めて）。
    fn close(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let terminals = tab.panes.clone();
        self.confirm_then(terminals, index + 1, window, cx, Self::close_terminals_now);
    }

    /// ⌘W / 分割した端末の ×: その端末だけを閉じる（最後の 1 つならタブごと）。
    fn close_pane(&mut self, tab: usize, pane: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self
            .tabs
            .get(tab)
            .and_then(|tab| tab.panes.get(pane))
            .cloned()
        else {
            return;
        };
        self.confirm_then(
            vec![terminal],
            tab + 1,
            window,
            cx,
            Self::close_terminals_now,
        );
    }

    /// 端末をドックから外す（分割は残りを詰める・空いたタブは消す）。全部無くなればドックを閉じる。
    /// 前にあったタブが残ればそのまま、消えたら 1 つ前のタブ（無ければ最初）を前にする。
    fn close_terminals_now(
        &mut self,
        targets: &[gpui::EntityId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old_active = self.active;
        let mut kept = Vec::new();
        for (old_index, mut tab) in std::mem::take(&mut self.tabs).into_iter().enumerate() {
            let focused = tab.focused_pane().entity_id();
            tab.panes
                .retain(|terminal| !targets.contains(&terminal.entity_id()));
            if tab.panes.is_empty() {
                continue;
            }
            tab.focused = tab
                .panes
                .iter()
                .position(|terminal| terminal.entity_id() == focused)
                .unwrap_or_else(|| tab.focused.min(tab.panes.len() - 1));
            kept.push(old_index);
            self.tabs.push(tab);
        }
        if self.tabs.is_empty() {
            self.active = 0;
            cx.emit(TerminalDockEvent::Dismissed);
        } else {
            self.active = match kept.iter().position(|index| *index == old_active) {
                Some(position) => position,
                None => kept
                    .iter()
                    .rposition(|index| *index < old_active)
                    .unwrap_or(0),
            };
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
        for index in 0..self.tabs.len() {
            let is_active = index == active;
            // アプリが付けたタイトル（OSC 0 / 2・`✳ Claude Code` など）。無ければ連番。分割したタブは
            // 並べた端末の名前を ` │ ` でつなぐ。
            let title = self.tabs[index]
                .panes
                .iter()
                .map(|terminal| {
                    terminal
                        .read(cx)
                        .display_title()
                        .map(str::to_string)
                        .unwrap_or_else(|| i18n::t!("terminal.tab_title", "n" => index + 1))
                })
                .collect::<Vec<_>>()
                .join(" │ ");
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
                        div()
                            .max_w(px(260.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(title)),
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
                    // 1 回目で切り替え、2 回目（ダブルクリック）で改名を頼む（O24・分割なら前にいる端末）。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            if event.click_count >= 2 {
                                if let Some(terminal) =
                                    this.tabs.get(index).map(|tab| tab.focused_pane().clone())
                                {
                                    cx.emit(TerminalDockEvent::RenameRequested(terminal));
                                }
                            } else {
                                this.switch_to(index, window, cx);
                            }
                        }),
                    ),
            );
        }
        let quick_commands = cx
            .try_global::<QuickCommands>()
            .map(|commands| commands.0.clone())
            .unwrap_or_default();
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
            // 横に分割（O24・⌘\）。前にあるタブの前にいる端末の右に新しい端末を開く。
            .when(!self.tabs.is_empty(), |header| {
                header.child(
                    div()
                        .id("term-split")
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(28.))
                        .h_full()
                        .text_color(theme.fg2)
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.fg0).bg(theme.bg1))
                        .child(
                            svg()
                                .path("icons/columns-2.svg")
                                .size(px(13.))
                                .text_color(theme.fg2),
                        )
                        .tooltip(Tooltip::text(
                            i18n::t!(
                                "terminal.split_tip",
                                "key" => keymap_core::keystroke_label_in(
                                    keymap_core::TERMINAL_CONTEXT,
                                    "cmd-\\"
                                )
                            ),
                            theme.clone(),
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.split(window, cx)),
                        ),
                )
            })
            // よく使うコマンド（O25）: 設定にある時だけ ▶。押すと下に帯を開く。
            .when(!quick_commands.is_empty(), |header| {
                let open = self.quick_open;
                header.child(
                    div()
                        .id("term-quick")
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(28.))
                        .h_full()
                        .text_size(px(10.))
                        .text_color(if open { theme.fg0 } else { theme.fg2 })
                        .when(open, |element| element.bg(theme.bg1))
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.fg0).bg(theme.bg1))
                        .child("▶")
                        .tooltip(Tooltip::text(i18n::t!("terminal.quick_tip"), theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                this.quick_open = !this.quick_open;
                                cx.notify();
                            }),
                        ),
                )
            })
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
        let body = match self.tabs.get(active) {
            Some(tab) if tab.panes.len() == 1 => div().flex_1().min_h_0().overflow_hidden().child(
                tab.panes[0]
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
            // 分割（O24）: 横に等分。それぞれに細い見出し（名前・×）を付け、前にいる端末の名前を濃くする。
            Some(tab) => {
                let mut row = div().flex_1().min_h_0().flex().flex_row();
                for (pane, terminal) in tab.panes.iter().enumerate() {
                    let focused = pane == tab.focused;
                    let name = terminal
                        .read(cx)
                        .display_title()
                        .map(SharedString::from)
                        .unwrap_or_else(|| {
                            SharedString::from(i18n::t!("terminal.pane_title", "n" => pane + 1))
                        });
                    row = row.child(
                        div()
                            .id(("term-pane", pane))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .when(pane > 0, |element| {
                                element.border_l_1().border_color(theme.border)
                            })
                            // 押した端末を前にする（打鍵は端末自身がフォーカスを取りに行く）。
                            .capture_any_mouse_down(cx.listener(move |this, _, _window, cx| {
                                this.set_focused_pane(active, pane, cx)
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(px(6.))
                                    .h(px(20.))
                                    .px(px(8.))
                                    .bg(if focused { theme.bg1 } else { theme.bg0 })
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .text_size(px(10.5))
                                    .text_color(if focused { theme.fg0 } else { theme.fg2 })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .id(("term-pane-close", pane))
                                            .flex_none()
                                            .text_color(theme.fg2)
                                            .cursor_pointer()
                                            .hover(|style| style.text_color(theme.fg0))
                                            .child("×")
                                            .tooltip(Tooltip::text(
                                                i18n::t!("terminal.pane_close_tip"),
                                                theme.clone(),
                                            ))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, window, cx| {
                                                    cx.stop_propagation();
                                                    this.close_pane(active, pane, window, cx);
                                                }),
                                            ),
                                    ),
                            )
                            .child(
                                div().flex_1().min_h_0().overflow_hidden().child(
                                    terminal
                                        .clone()
                                        .cached(StyleRefinement::default().size_full()),
                                ),
                            ),
                    );
                }
                row
            }
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
        // よく使うコマンドの帯（▶ で開閉・O25）: 押すと新しい端末で走らせる。
        let quick_strip = (self.quick_open && !quick_commands.is_empty()).then(|| {
            let mut strip = div()
                .id("term-quick-strip")
                .flex()
                .flex_none()
                .flex_wrap()
                .items_center()
                .gap(px(5.))
                .px(px(8.))
                .py(px(4.))
                .bg(theme.bg0)
                .border_b_1()
                .border_color(theme.border)
                .text_size(px(11.));
            for (index, command) in quick_commands.iter().enumerate() {
                strip = strip.child(
                    div()
                        .id(("term-quick-command", index))
                        .flex_none()
                        .px(px(7.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.border)
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg1).text_color(theme.fg0))
                        .child(SharedString::from(format!("▶ {}", command.name)))
                        .tooltip(Tooltip::text(command.command.clone(), theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.run_quick_command(index, window, cx)
                            }),
                        ),
                );
            }
            strip
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            // 端末にフォーカスがある時の ⌘T / ⌘W（Windows / Linux は Ctrl+Shift+T / W）。端末から
            // 上がってくる action をここで受ける＝裏のエディタのタブを閉じない（O24・C18）。
            .on_action(
                cx.listener(|this, _: &crate::actions::NewTab, window, cx| this.add(window, cx)),
            )
            // ⌘W は前にいる端末だけを閉じる（分割していなければタブごと）。
            .on_action(
                cx.listener(|this, _: &crate::actions::CloseTab, window, cx| {
                    let pane = this.tabs.get(this.active).map_or(0, |tab| tab.focused);
                    this.close_pane(this.active, pane, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::Split, window, cx| this.split(window, cx)),
            )
            .child(header)
            .children(quick_strip)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fish() -> TerminalShell {
        TerminalShell {
            program: "/opt/homebrew/bin/fish".to_string(),
            args: vec!["-l".to_string()],
        }
    }

    /// O25: 設定のシェルは手元の端末の既定だけを置き換える（Windows の pwsh 指定も手元）。
    #[test]
    fn the_chosen_shell_replaces_only_local_defaults() {
        let local = TerminalLaunch {
            cwd: Some(PathBuf::from("/work/necoder")),
            shell: None,
        };
        assert_eq!(
            local.clone().with_shell(Some(&fish())).shell,
            Some(("/opt/homebrew/bin/fish".to_string(), vec!["-l".to_string()]))
        );
        let windows_default = TerminalLaunch {
            cwd: Some(PathBuf::from(r"C:\work")),
            shell: Some(("pwsh".to_string(), Vec::new())),
        };
        assert_eq!(
            windows_default
                .with_shell(Some(&fish()))
                .shell
                .map(|(program, _)| program),
            Some("/opt/homebrew/bin/fish".to_string())
        );
        let remote = TerminalLaunch {
            cwd: None,
            shell: Some(("ssh".to_string(), vec!["-tt".to_string()])),
        };
        assert_eq!(
            remote.clone().with_shell(Some(&fish())),
            remote,
            "SSH 先は接続先のシェル"
        );
        let blank = TerminalShell {
            program: "  ".to_string(),
            args: Vec::new(),
        };
        assert_eq!(
            local.clone().with_shell(Some(&blank)),
            local,
            "空は OS の既定"
        );
        assert_eq!(local.clone().with_shell(None), local);
    }
}
