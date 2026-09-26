//! 終了の確認（O4・実行中のエージェントを ⌘Q や窓閉じで殺さない）。
//!
//! エージェント（ACP の子プロセス）も端末（PTY）もアプリ本体の子で、持ち主は窓（Workspace）。
//! ⌘Q なら全窓の分が、窓を閉じればその窓の分が止まる（**ほかの窓が開いていても**止まる）。
//! 本格的な daemon 化は別の仕事で、ここは安く効く方: **止まるものがある時だけ**
//! 「隠して（最小化して）動かし続ける / 終了する（閉じる）/ キャンセル」を聞く。
//! 何も止まらなければ今までどおり即終了する（即閉じる）。
//!
//! 「動かし続ける」は**アプリのプロセスが生きている間だけ**。終了・クラッシュ・更新の再起動では
//! 止まる（確認の文と MANUAL でそう書く・R15）。
//!
//! 数えるもの（⌘Q は全窓・窓閉じはその窓だけ。どちらも全プロジェクト・Chat を含む）: 実行中（Working）と
//! 承認待ち・質問待ち（Blocked）のスレッド、前面でシェル以外のプロセスが動いている端末
//! （`TerminalView::has_foreground_process`）。端末タブ 1 枚の × は terminal_view 側で同じ判定を使う。
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

/// 何をしようとして止めたか（数える範囲・文・「止める」ボタンの中身が変わる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitReason {
    /// ⌘Q（アプリの終了）。全窓の分を数える。
    Quit,
    /// 窓を閉じる。数えるのは**この窓の分だけ**（窓が閉じれば中のエージェントと端末も止まる。
    /// ほかの窓の分は止まらない）。`last` = ほかに workspace の窓が無い。
    CloseWindow { last: bool },
}

/// 「動かし続ける」を選んだ時に何をするか。どれもプロセスは止めない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutOfSight {
    /// アプリごと隠す（macOS。Dock のアイコンで戻る）。
    HideApp,
    /// 全窓を最小化する（`App::hide` が効かない Windows / Linux）。
    MinimizeAll,
    /// この窓だけ最小化する（ほかの窓は開いたまま使い続ける＝アプリごと隠すと巻き添えになる）。
    MinimizeThisWindow,
}

/// 何をしようとしたか + OS → 「動かし続ける」の中身。
pub(crate) fn out_of_sight(reason: QuitReason, mac: bool) -> OutOfSight {
    match reason {
        QuitReason::CloseWindow { last: false } => OutOfSight::MinimizeThisWindow,
        _ if mac => OutOfSight::HideApp,
        _ => OutOfSight::MinimizeAll,
    }
}

/// 確認ダイアログの状態（開いている間だけ Some）。`focus` は ⏎ / Esc の受け口。
pub(crate) struct QuitConfirmState {
    reason: QuitReason,
    work: RunningWork,
    focus: FocusHandle,
}

#[derive(Clone, Copy)]
enum QuitChoice {
    /// 既定（⏎）。隠す / 最小化する（[`out_of_sight`]）。どれもプロセスは止めない。
    KeepRunning,
    /// ⌘Q なら終了、窓閉じなら窓を閉じる（動いていたものは止まる）。
    Stop,
    Cancel,
}

/// Quit の入口（⌘Q・メニュー・Dock の「終了」）。動いているものがあり確認を切っていなければ、
/// 前面の窓に確認ダイアログを出す。そうでなければ即終了する。
pub fn request_quit(cx: &mut App) {
    // 既に終了の確認中なら、その窓を前に出すだけ（⌘Q の連打で 2 枚目を出さない）。
    // 窓閉じの確認（その窓の分だけを数えている）を出している最中の ⌘Q は、終了の確認（全窓の分）へ
    // 置き換える＝⌘Q なのに「この窓を閉じる」の範囲と文で答えさせない。
    let mut replaced = None;
    for handle in workspace_windows(cx) {
        let confirming = handle
            .read(cx)
            .ok()
            .and_then(|workspace| workspace.overlays.quit_confirm.as_ref())
            .map(|state| state.reason);
        match confirming {
            Some(QuitReason::Quit) => {
                cx.activate(true);
                if let Err(error) = handle.update(cx, |_, window, _| window.activate_window()) {
                    eprintln!("終了の確認を前に出せない: {error:#}");
                }
                return;
            }
            Some(QuitReason::CloseWindow { .. }) => {
                let dismissed = handle.update(cx, |workspace, _, cx| {
                    workspace.overlays.quit_confirm = None;
                    cx.notify();
                });
                match dismissed {
                    Ok(()) => replaced = Some(handle),
                    Err(error) => eprintln!("窓閉じの確認を閉じられない: {error:#}"),
                }
            }
            None => {}
        }
    }
    let work = running_work_in_all_windows(cx);
    if !quit_needs_confirmation(settings::get(cx).quit_confirmation(), work) {
        quit_now(cx);
        return;
    }
    let target = replaced
        .or_else(|| {
            cx.active_window()
                .and_then(|window| window.downcast::<Workspace>())
        })
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

/// OS 経由の窓閉じ（赤いボタン・Windows の × 等）の関所。**この窓で**止まるものがあれば
/// （ほかの窓が開いていても）閉じずに確認を出して `true`（＝閉じるのを止めた）を返す。
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

/// アプリ全体の「動かし続ける」（⌘Q・最後の窓）。macOS はアプリを隠す（Dock のアイコンで戻る）。
/// `App::hide` が効かない Windows / Linux は全窓を最小化する（タスクバーから戻る）。
/// どちらもプロセスは止めない。ほかの窓が残る窓閉じは、その窓だけ最小化する（`choose_quit`）。
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

    /// 窓を閉じる前の関所（OS 経由・自前 titlebar の × の共通）。**この窓で**止まるものがあれば、
    /// ほかの窓が開いていても確認を出して `true` を返す（呼び出し側は閉じない）。この窓で何も
    /// 動いていなければ、ほかの窓がどうであれ確認しない（ほかの窓の分は閉じても止まらない）。
    /// 「最後の窓か」は数える範囲ではなく、「動かし続ける」の中身（アプリごと隠すか・この窓だけ
    /// 最小化するか）と文だけを変える。
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
        let last = !cx
            .windows()
            .into_iter()
            .any(|other| other != this_window && other.downcast::<Workspace>().is_some());
        self.open_quit_confirm(QuitReason::CloseWindow { last }, work, window, cx);
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
            (QuitChoice::KeepRunning, reason) => {
                match out_of_sight(reason, cfg!(target_os = "macos")) {
                    OutOfSight::MinimizeThisWindow => window.minimize_window(),
                    // 全窓に触るので、この窓の update を抜けてから行う。
                    OutOfSight::HideApp | OutOfSight::MinimizeAll => {
                        cx.defer(keep_running_out_of_sight)
                    }
                }
            }
            (QuitChoice::Stop, QuitReason::Quit) => cx.defer(quit_now),
            (QuitChoice::Stop, QuitReason::CloseWindow { .. }) => {
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
    /// 文は 3 段: どの範囲が止まるか（全窓 / この窓）→ 隠す・最小化は何をするか → それが続くのは
    /// アプリが起動している間だけ（終了・クラッシュ・更新の再起動では止まる・R15）。
    pub(crate) fn render_quit_confirm(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.quit_confirm.as_ref()?;
        let theme = self.theme.clone();
        let keep = out_of_sight(state.reason, cfg!(target_os = "macos"));

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
        let title = match state.reason {
            QuitReason::Quit => i18n::t!("quit_confirm.title"),
            QuitReason::CloseWindow { .. } => i18n::t!("quit_confirm.title_window"),
        };
        let stops = match state.reason {
            QuitReason::Quit => i18n::t!("quit_confirm.stops_quit"),
            QuitReason::CloseWindow { last: false } => {
                i18n::t!("quit_confirm.stops_close_window")
            }
            QuitReason::CloseWindow { last: true } => i18n::t!("quit_confirm.stops_close_last"),
        };
        let (keep_label, keep_explained) = match keep {
            OutOfSight::HideApp => (
                i18n::t!("quit_confirm.keep_running_hide"),
                i18n::t!("quit_confirm.hide_explained"),
            ),
            OutOfSight::MinimizeAll | OutOfSight::MinimizeThisWindow => (
                i18n::t!("quit_confirm.keep_running_minimize"),
                i18n::t!("quit_confirm.minimize_explained"),
            ),
        };
        let stop_label = match state.reason {
            QuitReason::Quit => i18n::t!("quit_confirm.quit"),
            QuitReason::CloseWindow { .. } => i18n::t!("quit_confirm.close"),
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
                    .child(SharedString::from(title)),
            )
            .child(rows)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(11.))
                    .child(div().text_color(theme.fg1).child(SharedString::from(stops)))
                    .child(
                        div()
                            .text_color(theme.fg2)
                            .child(SharedString::from(keep_explained)),
                    )
                    .child(
                        div()
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("quit_confirm.lifetime"))),
                    ),
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

    /// 「動かし続ける」の中身。アプリごと隠すのは、ほかに窓が残らない時だけ（ほかの窓を
    /// 巻き添えにしない）。hide が効かない OS は最小化。
    #[test]
    fn keeping_work_running_hides_the_app_only_when_no_other_window_is_left() {
        for mac in [true, false] {
            assert_eq!(
                out_of_sight(QuitReason::CloseWindow { last: false }, mac),
                OutOfSight::MinimizeThisWindow,
                "ほかの窓が開いている窓閉じは、この窓だけ最小化（mac={mac}）"
            );
        }
        assert_eq!(out_of_sight(QuitReason::Quit, true), OutOfSight::HideApp);
        assert_eq!(
            out_of_sight(QuitReason::CloseWindow { last: true }, true),
            OutOfSight::HideApp
        );
        assert_eq!(
            out_of_sight(QuitReason::Quit, false),
            OutOfSight::MinimizeAll
        );
        assert_eq!(
            out_of_sight(QuitReason::CloseWindow { last: true }, false),
            OutOfSight::MinimizeAll
        );
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
            Some(QuitReason::CloseWindow { last: true })
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 窓 A・B（同じ設定・別々のプロジェクト）。本番と同じく OS の閉じる経路（should-close フック）を
    /// 入れておく（永続化なし＝DB に触れない）。エージェントは先張りしない（実プロセスを立てない）。
    fn two_windows(
        name: &str,
        cx: &mut gpui::TestAppContext,
    ) -> (WindowHandle<Workspace>, WindowHandle<Workspace>, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("necoder_quit_guard_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let mut open = |label: &str| {
            let project = root.join(label);
            std::fs::create_dir_all(&project).unwrap();
            cx.add_window(|window, cx| {
                let persistence = crate::persistence::WindowPersistence {
                    storage: None,
                    window_id: None,
                };
                crate::persistence::install_window_close_hook(window, cx, &persistence);
                Workspace::new(vec![project], Theme::dark(), None, cx)
            })
        };
        let first = open("a");
        let second = open("b");
        cx.run_until_parked();
        (first, second, root)
    }

    fn confirm_of(
        handle: WindowHandle<Workspace>,
        cx: &mut gpui::TestAppContext,
    ) -> Option<(QuitReason, RunningWork)> {
        handle
            .read_with(cx, |workspace, _| {
                workspace
                    .overlays
                    .quit_confirm
                    .as_ref()
                    .map(|state| (state.reason, state.work))
            })
            .unwrap()
    }

    /// R04: A で実行中と承認待ちのエージェント、B は何も動いていない。OS の閉じるボタンで
    /// B を閉じる時は確認しない。A を閉じる時は B が開いていても閉じずに確認し、数えるのは A の分だけ。
    /// 「閉じる」を選ぶと A だけが閉じ、B は残る。
    #[gpui::test]
    fn closing_a_busy_window_asks_even_while_another_window_is_open(cx: &mut gpui::TestAppContext) {
        let (busy, idle, root) = two_windows("busy_window", cx);
        busy.update(cx, |workspace, _window, cx| {
            let panel = workspace.project_sessions.sessions[0].agent_panel.clone();
            panel.update(cx, |panel, cx| {
                panel.new_thread(cx);
                // 先頭が実行中（Working）、2 本目が承認待ち（Blocked）。
                panel.debug_set_activities(cx);
            });
        })
        .unwrap();

        let mut idle_window = gpui::VisualTestContext::from_window(idle.into(), cx);
        assert!(
            idle_window.simulate_close(),
            "何も動いていない B は、A が動いていても確認なしで閉じる"
        );
        assert_eq!(confirm_of(idle, cx), None);

        let mut busy_window = gpui::VisualTestContext::from_window(busy.into(), cx);
        assert!(
            !busy_window.simulate_close(),
            "B が開いていても、動いている A は閉じずに確認"
        );
        let (reason, running) = confirm_of(busy, cx).expect("A に確認が開く");
        assert_eq!(reason, QuitReason::CloseWindow { last: false });
        assert_eq!(running, work(1, 1, 0), "数えるのは A の分だけ");
        assert!(!is_quitting());

        busy.update(cx, |workspace, window, cx| {
            workspace.choose_quit(QuitChoice::Stop, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
        let windows = cx.windows();
        assert!(!windows.contains(&busy.into()), "「閉じる」で A が閉じる");
        assert!(windows.contains(&idle.into()), "B は残る");
        assert!(!is_quitting(), "窓を閉じただけで、アプリは終了しない");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 端末だけが動いている窓も同じ（前面でプロセスが動くターミナル = 窓を閉じると止まるもの）。
    /// 自前 titlebar の ×（Windows / Linux）と同じ関所を直接叩く。窓閉じの確認中に ⌘Q したら、
    /// 終了の確認（全窓の分を足して数える）に置き換わる。
    #[gpui::test]
    fn a_busy_terminal_guards_only_its_own_window(cx: &mut gpui::TestAppContext) {
        let (busy, idle, root) = two_windows("busy_terminal", cx);
        busy.update(cx, |workspace, _window, cx| {
            let dock = workspace.project_sessions.sessions[0].terminal_dock.clone();
            let terminal = dock.update(cx, |dock, cx| dock.ensure_active_test(cx));
            terminal.update(cx, |terminal, _| terminal.set_test_foreground_process(true));
        })
        .unwrap();

        let blocked = idle
            .update(cx, |workspace, window, cx| {
                workspace.guard_window_close(window, cx)
            })
            .unwrap();
        assert!(!blocked, "端末が動いているのは A だけ = B は確認しない");
        let blocked = busy
            .update(cx, |workspace, window, cx| {
                workspace.guard_window_close(window, cx)
            })
            .unwrap();
        assert!(blocked);
        assert_eq!(
            confirm_of(busy, cx),
            Some((QuitReason::CloseWindow { last: false }, work(0, 0, 1)))
        );

        // B にも実行中のエージェントを足してから、A の窓閉じの確認を出したまま ⌘Q: 同じ窓の確認が
        // 終了の確認に置き換わり、全窓の分（A の端末 + B のエージェント）を数える。2 枚目は出さない。
        idle.update(cx, |workspace, _window, cx| {
            let panel = workspace.project_sessions.sessions[0].agent_panel.clone();
            panel.update(cx, |panel, cx| panel.debug_set_activities(cx));
        })
        .unwrap();
        cx.update(request_quit);
        assert_eq!(
            confirm_of(busy, cx),
            Some((QuitReason::Quit, work(1, 0, 1)))
        );
        assert_eq!(confirm_of(idle, cx), None);
        assert!(!is_quitting());
        busy.update(cx, |workspace, window, cx| {
            workspace.choose_quit(QuitChoice::Cancel, window, cx)
        })
        .unwrap();
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
