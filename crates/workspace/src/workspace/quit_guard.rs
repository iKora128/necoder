//! 終了の確認（O4・実行中のエージェントを ⌘Q や窓閉じで殺さない）。
//!
//! エージェント（ACP の子プロセス）も端末（PTY）もアプリ本体の子なので、⌘Q や最後の窓を閉じると
//! 一緒に止まる。本格的な daemon 化は別の仕事で、ここは安く効く方: **動いているものがある時だけ**
//! 「隠して動かし続ける / 終了する / キャンセル」を聞く。何も動いていなければ今までどおり即終了する。
//!
//! 数えるもの（全窓・全プロジェクト・Chat も）: 実行中（Working）と承認待ち・質問待ち（Blocked）の
//! スレッド、前面でシェル以外のプロセスが動いている端末（`TerminalView::has_foreground_process`）。
//!
//! 入口は 3 つ: [`request_quit`]（⌘Q・メニューの「終了」・macOS の Dock / AppleScript の「終了」は
//! main のフックがここへ回す）、OS 経由の窓閉じ（`install_window_close_hook`）、自前 titlebar の ×。

use crate::persistence::{is_quitting, mark_quitting};
use crate::workspace::*;
use gpui::WindowHandle;
use settings_core::QuitConfirmation;

/// アプリ全体で共有する DB（main が起動時に置く）。窓を経由せずに引く必要がある 2 か所 —
/// 窓が 1 つも無い時の ⌘Q（窓セッションの整理）と、Dock からの開き直し — が使う。
pub struct AppStorage(pub Option<storage::Storage>);

impl gpui::Global for AppStorage {}

/// いま動いているもの（確認ダイアログの本文と「聞くか」の判定に使う）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RunningWork {
    /// 実行中（Working）のスレッド。
    pub(crate) working: usize,
    /// 承認待ち・質問待ち（Blocked）のスレッド。
    pub(crate) waiting: usize,
    /// 前面でプロセスが動いている端末。
    pub(crate) terminals: usize,
}

impl RunningWork {
    pub(crate) fn is_idle(&self) -> bool {
        self.working == 0 && self.waiting == 0 && self.terminals == 0
    }

    fn add(&mut self, other: RunningWork) {
        self.working += other.working;
        self.waiting += other.waiting;
        self.terminals += other.terminals;
    }
}

/// 確認が要るか。設定で切っていれば聞かない・何も動いていなければ聞かない（今までどおり即終了）。
pub(crate) fn quit_needs_confirmation(confirmation: QuitConfirmation, work: RunningWork) -> bool {
    confirmation == QuitConfirmation::WhenRunning && !work.is_idle()
}

/// 何をしようとして止めたか（「止める」ボタンの言い方と中身が変わる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitReason {
    /// ⌘Q（アプリの終了）。
    Quit,
    /// 最後の窓を閉じる（窓が閉じれば中のエージェントと端末も止まる）。
    CloseLastWindow,
    /// ほかの窓が残る状態で、動いているものがある窓を閉じる（R04）。止まるのはこの窓の分だけで、
    /// 「動かし続ける」はアプリ全体を隠さず、この窓だけを最小化する。
    CloseWindow,
}

/// 確認ダイアログの状態（開いている間だけ Some）。`focus` は ⏎ / Esc の受け口。
pub(crate) struct QuitConfirmState {
    reason: QuitReason,
    work: RunningWork,
    focus: FocusHandle,
}

#[derive(Clone, Copy)]
enum QuitChoice {
    /// 既定（⏎）。macOS はアプリを隠し、それ以外は窓を最小化する。どちらもプロセスは止めない。
    KeepRunning,
    /// ⌘Q なら終了、窓閉じなら窓を閉じる（動いていたものは止まる）。
    Stop,
    Cancel,
}

/// Quit の入口（⌘Q・メニュー・Dock の「終了」）。動いているものがあり確認を切っていなければ、
/// 前面の窓に確認ダイアログを出す。そうでなければ即終了する。
pub fn request_quit(cx: &mut App) {
    // 既に確認中なら、その窓を前に出すだけ（⌘Q の連打で 2 枚目を出さない）。
    for handle in workspace_windows(cx) {
        let confirming = handle
            .read(cx)
            .is_ok_and(|workspace| workspace.overlays.quit_confirm.is_some());
        if confirming {
            cx.activate(true);
            if let Err(error) = handle.update(cx, |_, window, _| window.activate_window()) {
                eprintln!("終了の確認を前に出せない: {error:#}");
            }
            return;
        }
    }
    let work = running_work_in_all_windows(cx);
    if !quit_needs_confirmation(settings::get(cx).quit_confirmation(), work) {
        quit_now(cx);
        return;
    }
    let target = cx
        .active_window()
        .and_then(|window| window.downcast::<Workspace>())
        .or_else(|| workspace_windows(cx).into_iter().last());
    let Some(target) = target else {
        quit_now(cx);
        return;
    };
    // 隠していた時（「隠して動かし続ける」の後の Dock の「終了」）も確認が見えるよう前へ出す。
    cx.activate(true);
    let opened = target.update(cx, |workspace, window, cx| {
        workspace.open_quit_confirm(QuitReason::Quit, work, window, cx);
        window.activate_window();
    });
    if let Err(error) = opened {
        // 確認を出せないのに終了も止めると、⌘Q が効かないアプリになる。頼まれたとおり終了する。
        eprintln!("終了の確認を出せない（そのまま終了）: {error:#}");
        quit_now(cx);
    }
}

/// 本当に終了する（確認済み・確認不要）。hot exit を片付け、窓セッションは今開いている窓を
/// 「生存」のまま残し、それ以外の生存行（引数付き起動で残った過去の窓）は閉じ扱いにしてから
/// OS に終了を頼む＝次回の復元は「最後に終了した時の窓の集合」。
pub fn quit_now(cx: &mut App) {
    mark_quitting();
    let mut live_ids = Vec::new();
    for handle in workspace_windows(cx) {
        match handle.update(cx, |workspace, _window, _cx| {
            workspace.prepare_quit();
            workspace.window_session_id()
        }) {
            Ok(window_id) => live_ids.extend(window_id),
            Err(error) => eprintln!("終了前の後始末ができない窓がある: {error:#}"),
        }
    }
    let storage = cx
        .try_global::<AppStorage>()
        .and_then(|global| global.0.clone());
    if let Some(storage) = storage {
        if let Err(error) = storage.retain_window_sessions(&live_ids) {
            eprintln!("窓セッションの整理に失敗: {error:#}");
        }
    }
    cx.quit();
}

/// OS 経由の窓閉じ（赤いボタン・Windows の × 等）の関所。**この窓で**動いているものがあれば
/// 閉じずに確認を出して `true`（＝閉じるのを止めた）を返す。ほかの窓が開いているかどうかは
/// 確認の文面を変えるだけで、聞くかどうかには関わらない（R04: 別の窓があるだけで素通しすると、
/// 実行中の窓を確認なしで閉じられた）。
///
/// `on_window_should_close` の中（この窓を update 中・Workspace 自体は未借用）から呼ぶ。
pub(crate) fn intercept_window_close(window: &mut Window, cx: &mut App) -> bool {
    let Some(Some(workspace)) = window.root::<Workspace>() else {
        return false;
    };
    workspace.update(cx, |workspace, cx| workspace.guard_window_close(window, cx))
}

fn workspace_windows(cx: &App) -> Vec<WindowHandle<Workspace>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<Workspace>())
        .collect()
}

fn running_work_in_all_windows(cx: &App) -> RunningWork {
    let mut total = RunningWork::default();
    for handle in workspace_windows(cx) {
        if let Ok(workspace) = handle.read(cx) {
            total.add(workspace.running_work(cx));
        }
    }
    total
}

/// 「動かし続ける」の実体。macOS はアプリを隠す（Dock のアイコンで戻る）。`App::hide` が効かない
/// Windows / Linux は全窓を最小化する（タスクバーから戻る）。どちらもプロセスは止めない。
fn keep_running_out_of_sight(cx: &mut App) {
    if cfg!(target_os = "macos") {
        cx.hide();
        return;
    }
    for handle in workspace_windows(cx) {
        if let Err(error) = handle.update(cx, |_, window, _| window.minimize_window()) {
            eprintln!("窓を最小化できない: {error:#}");
        }
    }
}

impl Workspace {
    /// この窓で動いているもの（全プロジェクト + Chat・Fleet で足したエージェントと端末も）。
    pub(crate) fn running_work(&self, cx: &App) -> RunningWork {
        let mut work = RunningWork::default();
        let sessions = self
            .project_sessions
            .sessions
            .iter()
            .chain(self.project_sessions.chat.as_ref());
        for session in sessions {
            for panel in &session.fleet_agents {
                for (_, _, activity) in panel.read(cx).beacons() {
                    match activity {
                        agent_panel::ThreadActivity::Working => work.working += 1,
                        agent_panel::ThreadActivity::Blocked => work.waiting += 1,
                        _ => {}
                    }
                }
            }
            for dock in [&session.terminal_dock, &session.tests_dock] {
                work.terminals += dock.read(cx).busy_terminal_count(cx);
            }
        }
        work
    }

    /// 窓を閉じる前の関所（OS 経由・自前 titlebar の × の共通）。**この窓で**動いているものがあれば
    /// 確認を出して `true` を返す（呼び出し側は閉じない）。数えるのはこの窓の分だけなので、
    /// 何も動いていない窓は、ほかの窓で何が動いていても確認なしで閉じる。
    pub(crate) fn guard_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if is_quitting() {
            return false;
        }
        let work = self.running_work(cx);
        if !quit_needs_confirmation(settings::get(cx).quit_confirmation(), work) {
            return false;
        }
        let this_window = window.window_handle();
        let other_windows = cx
            .windows()
            .into_iter()
            .any(|other| other != this_window && other.downcast::<Workspace>().is_some());
        let reason = if other_windows {
            QuitReason::CloseWindow
        } else {
            QuitReason::CloseLastWindow
        };
        self.open_quit_confirm(reason, work, window, cx);
        true
    }

    pub(crate) fn open_quit_confirm(
        &mut self,
        reason: QuitReason,
        work: RunningWork,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.overlays.quit_confirm = Some(QuitConfirmState {
            reason,
            work,
            focus,
        });
        cx.notify();
    }

    fn choose_quit(&mut self, choice: QuitChoice, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.quit_confirm.take() else {
            return;
        };
        cx.notify();
        match (choice, state.reason) {
            (QuitChoice::Cancel, _) => {}
            // ほかの窓は見えたままなので、アプリごと隠さずこの窓だけを最小化する。
            (QuitChoice::KeepRunning, QuitReason::CloseWindow) => window.minimize_window(),
            // 全窓に触るので、この窓の update を抜けてから行う。
            (QuitChoice::KeepRunning, _) => cx.defer(keep_running_out_of_sight),
            (QuitChoice::Stop, QuitReason::Quit) => cx.defer(quit_now),
            (QuitChoice::Stop, QuitReason::CloseLastWindow | QuitReason::CloseWindow) => {
                self.mark_window_closed();
                window.remove_window();
            }
        }
    }

    fn on_quit_confirm_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "enter" => self.choose_quit(QuitChoice::KeepRunning, window, cx),
            "escape" => self.choose_quit(QuitChoice::Cancel, window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// 中央モーダル。**何が止まるかを数えて見せる**のが主役（worktree 削除の確認と同じ器）。
    /// 既定（⏎）は「動かし続ける」＝うっかりの ⌘Q で何も失わない側。
    pub(crate) fn render_quit_confirm(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.quit_confirm.as_ref()?;
        let theme = self.theme.clone();
        let mac = cfg!(target_os = "macos");

        let mut rows = div().flex().flex_col().gap(px(4.));
        for (count, key) in [
            (state.work.working, "quit_confirm.working"),
            (state.work.waiting, "quit_confirm.waiting"),
            (state.work.terminals, "quit_confirm.terminals"),
        ] {
            if count == 0 {
                continue;
            }
            rows = rows.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(7.))
                    .text_size(px(11.5))
                    .text_color(theme.fg1)
                    .child(div().flex_none().w(px(12.)).child("●"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(i18n::t!(key, "n" => count))),
                    ),
            );
        }
        let detail = match (state.reason, mac) {
            (QuitReason::Quit, true) => i18n::t!("quit_confirm.detail_quit_hide"),
            (QuitReason::Quit, false) => i18n::t!("quit_confirm.detail_quit_minimize"),
            (QuitReason::CloseLastWindow, true) => i18n::t!("quit_confirm.detail_close_hide"),
            (QuitReason::CloseLastWindow, false) => {
                i18n::t!("quit_confirm.detail_close_minimize")
            }
            (QuitReason::CloseWindow, _) => i18n::t!("quit_confirm.detail_close_window"),
        };
        // この窓だけを閉じる時は、どの OS でも「この窓を最小化」（アプリは隠さない）。
        let keep_label = if mac && state.reason != QuitReason::CloseWindow {
            i18n::t!("quit_confirm.keep_running_hide")
        } else {
            i18n::t!("quit_confirm.keep_running_minimize")
        };
        let stop_label = match state.reason {
            QuitReason::Quit => i18n::t!("quit_confirm.quit"),
            QuitReason::CloseLastWindow | QuitReason::CloseWindow => i18n::t!("quit_confirm.close"),
        };

        // 既定のボタンだけ面を持たせる（色相は使わない・§1.3）。止める側は worktree 削除と同じ err。
        let button = |id: &'static str, label: String, kind: QuitChoice, theme: &Theme| {
            let (border, text, fill) = match kind {
                QuitChoice::KeepRunning => (theme.fg2, theme.fg0, Some(theme.bg3)),
                QuitChoice::Stop => (theme.err.alpha(0.7), theme.err, None),
                QuitChoice::Cancel => (theme.border, theme.fg1, None),
            };
            let hover = theme.bg3;
            div()
                .id(id)
                .px(px(14.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(border)
                .when_some(fill, |element, fill| element.bg(fill))
                .text_size(px(12.))
                .text_color(text)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(SharedString::from(label))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.choose_quit(kind, window, cx);
                    }),
                )
        };

        let card = div()
            .w(px(440.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(10.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(12.),
                gpui::hsla(0., 0., 0., 0.5),
            )
            .blur_radius(px(28.))])
            .track_focus(&state.focus)
            .on_key_down(cx.listener(Self::on_quit_confirm_key_down))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!("quit_confirm.title"))),
            )
            .child(rows)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(detail)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(button(
                        "quit-confirm-cancel",
                        i18n::t!("quit_confirm.cancel"),
                        QuitChoice::Cancel,
                        &theme,
                    ))
                    .child(button(
                        "quit-confirm-stop",
                        stop_label,
                        QuitChoice::Stop,
                        &theme,
                    ))
                    .child(button(
                        "quit-confirm-keep",
                        format!("{keep_label}  ⏎"),
                        QuitChoice::KeepRunning,
                        &theme,
                    )),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("quit_confirm.setting_hint"))),
            );

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.45))
                // 背景クリックはキャンセル（止める操作を「外して押した」で実行しない）。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.choose_quit(QuitChoice::Cancel, window, cx)
                    }),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(working: usize, waiting: usize, terminals: usize) -> RunningWork {
        RunningWork {
            working,
            waiting,
            terminals,
        }
    }

    #[test]
    fn nothing_running_quits_without_asking() {
        assert!(!quit_needs_confirmation(
            QuitConfirmation::WhenRunning,
            RunningWork::default()
        ));
    }

    #[test]
    fn any_running_agent_or_terminal_asks_first() {
        for running in [work(1, 0, 0), work(0, 1, 0), work(0, 0, 1), work(2, 1, 3)] {
            assert!(
                quit_needs_confirmation(QuitConfirmation::WhenRunning, running),
                "{running:?} は聞く"
            );
        }
    }

    #[test]
    fn turning_the_setting_off_never_asks() {
        assert!(!quit_needs_confirmation(
            QuitConfirmation::Never,
            work(3, 2, 1)
        ));
    }

    #[test]
    fn running_work_adds_up_across_windows() {
        let mut total = work(1, 0, 2);
        total.add(work(0, 1, 1));
        assert_eq!(total, work(1, 1, 3));
        assert!(!total.is_idle());
    }

    /// 窓 1 つ + 設定（`extra` を settings.json に足す）。エージェントは先張りしない（実プロセスを立てない）。
    fn workspace_window<'a>(
        name: &str,
        extra: &str,
        cx: &'a mut gpui::TestAppContext,
    ) -> (Entity<Workspace>, &'a mut gpui::VisualTestContext, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("necoder_quit_guard_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            format!(r#"{{"onboarded":true,"agent_prewarm":false{extra}}}"#),
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        cx.run_until_parked();
        (workspace, cx, root)
    }

    fn start_an_agent(workspace: &Entity<Workspace>, cx: &mut gpui::VisualTestContext) {
        workspace.update_in(cx, |workspace, _window, cx| {
            let panel = workspace.project_sessions.sessions[0].agent_panel.clone();
            // 先頭スレッドが実行中（Working）になる。
            panel.update(cx, |panel, cx| panel.debug_set_activities(cx));
        });
    }

    /// 実行中のスレッドがある窓で ⌘Q → 終了せずに確認が開き、数が載る。キャンセルで閉じる。
    #[gpui::test]
    fn quitting_with_a_running_agent_opens_the_confirm(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = workspace_window("running", "", cx);
        start_an_agent(&workspace, cx);

        cx.cx.update(request_quit);
        let opened = workspace.read_with(cx, |workspace, _| {
            workspace
                .overlays
                .quit_confirm
                .as_ref()
                .map(|state| (state.reason, state.work))
        });
        let (reason, running) = opened.expect("動いているスレッドがあるので確認が開く");
        assert_eq!(reason, QuitReason::Quit);
        assert_eq!(running.working, 1, "{running:?}");
        assert!(!is_quitting(), "確認中は終了していない");

        // もう一度 ⌘Q しても 2 枚目は出さない（同じ状態のまま）。
        cx.cx.update(request_quit);
        assert!(workspace.read_with(cx, |workspace, _| workspace.overlays.quit_confirm.is_some()));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.choose_quit(QuitChoice::Cancel, window, cx)
        });
        assert!(workspace.read_with(cx, |workspace, _| workspace.overlays.quit_confirm.is_none()));
        assert!(!is_quitting());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 最後の窓を閉じる時も同じ関所。動いていれば閉じずに確認（理由は「窓を閉じる」）、
    /// 何も動いていなければ素通し（今までどおり閉じる）。
    #[gpui::test]
    fn closing_the_last_window_is_guarded_only_while_something_runs(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = workspace_window("close", "", cx);
        let blocked = workspace.update_in(cx, |workspace, window, cx| {
            workspace.guard_window_close(window, cx)
        });
        assert!(!blocked, "何も動いていない窓は確認なしで閉じる");

        start_an_agent(&workspace, cx);
        let blocked = workspace.update_in(cx, |workspace, window, cx| {
            workspace.guard_window_close(window, cx)
        });
        assert!(blocked, "動いている窓は閉じずに確認");
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace
                .overlays
                .quit_confirm
                .as_ref()
                .map(|state| state.reason)),
            Some(QuitReason::CloseLastWindow)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// R04: ほかの窓が開いていても、動いている窓を閉じる時は確認する（文面は「この窓」）。
    /// 何も動いていない窓は、ほかの窓で何が動いていても確認なしで閉じる。
    #[gpui::test]
    fn closing_a_running_window_is_guarded_even_with_other_windows_open(
        cx: &mut gpui::TestAppContext,
    ) {
        let root =
            std::env::temp_dir().join(format!("necoder_quit_guard_multi_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let projects = [root.join("a"), root.join("b")];
        for project in &projects {
            std::fs::create_dir_all(project).unwrap();
        }
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let running = cx.add_window(|_window, cx| {
            Workspace::new(vec![projects[0].clone()], Theme::dark(), None, cx)
        });
        let idle = cx.add_window(|_window, cx| {
            Workspace::new(vec![projects[1].clone()], Theme::dark(), None, cx)
        });
        cx.run_until_parked();
        running
            .update(cx, |workspace, _window, cx| {
                let panel = workspace.project_sessions.sessions[0].agent_panel.clone();
                panel.update(cx, |panel, cx| panel.debug_set_activities(cx));
            })
            .unwrap();

        let blocked = idle
            .update(cx, |workspace, window, cx| {
                workspace.guard_window_close(window, cx)
            })
            .unwrap();
        assert!(
            !blocked,
            "何も動いていない窓は、ほかの窓が動いていても確認なしで閉じる"
        );

        let blocked = running
            .update(cx, |workspace, window, cx| {
                workspace.guard_window_close(window, cx)
            })
            .unwrap();
        assert!(blocked, "ほかの窓が開いていても、動いている窓は閉じずに確認");
        let reason = running
            .update(cx, |workspace, _window, _cx| {
                workspace
                    .overlays
                    .quit_confirm
                    .as_ref()
                    .map(|state| state.reason)
            })
            .unwrap();
        assert_eq!(
            reason,
            Some(QuitReason::CloseWindow),
            "止まるのはこの窓の分だけ"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 設定で切っていれば、動いていても聞かない（窓閉じの関所も素通し）。
    #[gpui::test]
    fn the_setting_turns_the_guard_off(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = workspace_window("never", r#","confirm_quit":"never""#, cx);
        start_an_agent(&workspace, cx);
        let blocked = workspace.update_in(cx, |workspace, window, cx| {
            workspace.guard_window_close(window, cx)
        });
        assert!(!blocked);
        assert!(workspace.read_with(cx, |workspace, _| workspace.overlays.quit_confirm.is_none()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
