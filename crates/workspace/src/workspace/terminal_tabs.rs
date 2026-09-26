//! エディタ領域の端末タブ（O24・C03 の「異種タブの混在」）。下ドックと同じ端末を、ファイルのタブと
//! 同じ列に開く（VSCode の「エディタ領域にターミナルを作成」「ターミナルをエディタ領域へ移動」）。
//!
//! - PTY は session の `terminal_dock` が id で持つ（Fleet の Task に置いた端末と同じ
//!   `TerminalDock::start_session` / `detach_active`）。テーマ・プロジェクト色・リンクのクリック・
//!   ⌘Q の確認（O4）は下ドックの端末と同じに効き、タブは見せ方だけを持つ。
//! - 一時タブ（窓セッションに残さない・⌘⇧T で戻さない）。ブランチを切り替えてファイルのタブを
//!   開き直しても端末のタブは残す（シェルは動き続けている）。
//! - 閉じると端末も終わる。前面でプロセスが動いていれば閉じる前に確かめる（何枚まとめて閉じても 1 回）。
//! - 端末の中の ⌘W / ⌘T はこのタブを閉じる / エディタ領域に新しい端末を開く（主ペインが受ける）。
//!   ⌘\（分割）は下ドックの機能なので、エディタ領域では案内だけ出す。

use crate::workspace::web_preview_view::{truncate_label, TAB_LABEL_MAX_CHARS};
use crate::workspace::*;
use gpui::{PromptButton, PromptLevel};
use terminal_view::TerminalEvent;

/// 端末タブの鍵（タブの同一判定）。ファイルのパスとも Web タブの URL とも重ならない。
pub(crate) fn terminal_tab_key(session: u64) -> PathBuf {
    PathBuf::from(format!("necoder-terminal://{session}"))
}

impl Workspace {
    /// パレット「ターミナル: エディタ領域に新しく開く」・エディタ領域の端末の中の ⌘T。
    pub(crate) fn new_terminal_tab(
        &mut self,
        _: &NewTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.prepare_terminal_tab(cx) {
            return;
        }
        let session = self.chrome.work_layout.allocate();
        let view = self
            .terminal_dock
            .update(cx, |dock, cx| dock.start_session(session, cx));
        self.push_terminal_tab(view, session, window, cx);
    }

    /// パレット「ターミナル: エディタ領域へ移す」: 下ドックの前にいる端末を（止めずに）エディタ領域の
    /// タブにする。下ドックに端末が無ければ新しく開く。最後の 1 つを移したら下ドックは畳む。
    pub(crate) fn move_terminal_to_editor(
        &mut self,
        _: &MoveTerminalToEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.prepare_terminal_tab(cx) {
            return;
        }
        let session = self.chrome.work_layout.allocate();
        let (view, dock_has_tabs) = self.terminal_dock.update(cx, |dock, cx| {
            let view = dock.detach_active(session, cx);
            (view, dock.has_tabs())
        });
        if !dock_has_tabs {
            self.chrome.show_bottom = false;
        }
        self.push_terminal_tab(view, session, window, cx);
    }

    /// タブの右クリック「下ドックへ移す」: 端末は止めずに下ドックのタブへ戻し、下ドックを開く。
    pub(crate) fn move_terminal_tab_to_dock(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, session)) = self.tabs.get(index).and_then(EditorTab::terminal) else {
            return;
        };
        // 先にドックへ戻す（`detached` から抜ける）ので、タブを閉じても端末は止まらない。
        self.terminal_dock
            .update(cx, |dock, cx| dock.attach_session(session, window, cx));
        self.close_tab_now(index, window, cx);
        self.chrome.show_bottom = true;
        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    /// 端末タブの名前: 人が付けた名前 → シェルのタイトル（OSC 0 / 2）→「ターミナル N」
    /// （N はエディタ領域の端末タブの中の順）。
    pub(crate) fn terminal_tab_label(&self, index: usize, cx: &App) -> String {
        let Some((view, _)) = self.tabs.get(index).and_then(EditorTab::terminal) else {
            return String::new();
        };
        if let Some(title) = view.read(cx).display_title() {
            return truncate_label(title, TAB_LABEL_MAX_CHARS);
        }
        let number = self.tabs[..index]
            .iter()
            .filter(|tab| tab.is_terminal())
            .count()
            + 1;
        i18n::t!("terminal.tab_title", "n" => number)
    }

    /// タブを閉じる（⌘W・×・タブメニュー）。前面でプロセスが動いている端末のタブは、閉じる前に
    /// 確かめる（何枚あっても 1 回・下ドックと同じ OS のダイアログ）。それ以外はすぐ閉じる。
    /// 答えを待つ間に並びが変わってもよいよう、確かめる端末は添字でなく id で覚える。
    pub(crate) fn close_tabs_asking(
        &mut self,
        mut indices: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        indices.sort_unstable();
        indices.dedup();
        let mut busy: Vec<(u64, String)> = Vec::new();
        // 後ろから閉じる（手前のタブの添字はずれない＝端末の番号もずれない）。
        for index in indices.into_iter().rev() {
            let Some(tab) = self.tabs.get(index) else {
                continue;
            };
            match tab.terminal() {
                Some((view, session)) if view.read(cx).has_foreground_process() => {
                    busy.push((session, self.terminal_tab_label(index, cx)));
                }
                _ => self.close_tab_now(index, window, cx),
            }
        }
        if busy.is_empty() {
            return;
        }
        let message = match busy.as_slice() {
            [(_, name)] => i18n::t!("terminal.close_busy_named_title", "name" => name),
            _ => i18n::t!("terminal.close_busy_many_title", "n" => busy.len()),
        };
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
        let sessions: Vec<u64> = busy.into_iter().map(|(session, _)| session).collect();
        cx.spawn_in(window, async move |workspace, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let closed = workspace.update_in(cx, |workspace, window, cx| {
                workspace.close_terminal_tabs_now(&sessions, window, cx)
            });
            if let Err(error) = closed {
                eprintln!("端末のタブを閉じられない（窓が既に無い）: {error:#}");
            }
        })
        .detach();
    }

    /// id の端末のタブを確かめずに閉じる（確かめた後）。
    fn close_terminal_tabs_now(
        &mut self,
        sessions: &[u64],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for index in (0..self.tabs.len()).rev() {
            if self.tabs[index]
                .terminal()
                .is_some_and(|(_, session)| sessions.contains(&session))
            {
                self.close_tab_now(index, window, cx);
            }
        }
    }

    /// 端末の中の ⌘\（分割）。分割は下ドックの機能なので、エディタ領域では移し方を案内する。
    pub(crate) fn explain_terminal_tab_split(&mut self, cx: &mut Context<Self>) {
        let color = self.accent();
        self.push_toast(i18n::t!("terminal.tab_split_hint").into(), color, cx);
    }

    /// 端末タブを開く前の支度。Chat にはプロジェクトが無い（端末の場所が決まらない）ので断る。
    /// 設定ホーム・AI 全画面・Fleet は畳んで、エディタ領域を前に出す（Web タブと同じ）。
    fn prepare_terminal_tab(&mut self, cx: &mut Context<Self>) -> bool {
        if self.chat_mode() {
            let color = self.accent();
            self.push_toast(i18n::t!("terminal.tab_needs_project").into(), color, cx);
            return false;
        }
        self.chrome.show_settings = false;
        self.exit_agent_full_screen(cx);
        self.chrome.fleet_mode = false;
        self.agent_active = false;
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        true
    }

    fn push_terminal_tab(
        &mut self,
        view: Entity<TerminalView>,
        session: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 名前が変わった時だけタブ列を描き直す（出力のたびに窓全体を描き直さない）。
        let title = cx.subscribe(&view, |_, _, event: &TerminalEvent, cx| {
            if matches!(event, TerminalEvent::TitleChanged) {
                cx.notify();
            }
        });
        self.tabs.push(EditorTab {
            path: terminal_tab_key(session),
            content: TabContent::Terminal {
                view: view.clone(),
                session,
                _title: title,
            },
            transient: true,
            pinned: false,
            preview: false,
        });
        self.active_tab = self.tabs.len() - 1;
        let handle = view.read(cx).focus_handle();
        window.focus(&handle, cx);
        self.sync_active_slot();
        self.save_state(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O24: 端末をエディタ領域のタブで開いてファイルのタブと並べる。端末の中の ⌘T / ⌘W / ⌘\ は
    /// 主ペインが受け、下ドックとは止めずに行き来する。タブを開き直しても（ブランチの切り替え）残る。
    /// O24: 端末のタブが前にあっても、選んでいたファイルのタブがブランチの切り替え（ファイルのタブの開き
    /// 直し）の後も選ばれる。覚える位置はファイルの並びの中の位置で、端末の数を 2 度足さない。
    #[gpui::test]
    fn the_selected_file_is_kept_when_tabs_reopen_behind_terminals(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_terminal_tab_order_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        std::fs::write(&a, "a\n").unwrap();
        std::fs::write(&b, "b\n").unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace
                .terminal_dock
                .update(cx, |dock, _cx| dock.use_test_terminals());
            workspace.open_file_sync(a.clone(), window, cx);
            workspace.open_file_sync(b.clone(), window, cx);
            for terminals in 1..=2 {
                workspace.new_terminal_tab(&NewTerminalTab, window, cx);
                // 開き直すと端末のタブが前・ファイルがその後ろ（[端末…, a, b]）。
                workspace.open_slot_files(window, cx);
                assert_eq!(workspace.tabs.len(), terminals + 2);
                let position_of_a = workspace
                    .tabs
                    .iter()
                    .position(|tab| tab.path == a)
                    .expect("a のタブ");
                workspace.select_tab(position_of_a, window, cx);
                assert_eq!(
                    workspace.active_slot().map(|slot| slot.active_file),
                    Some(0),
                    "ファイルの並びの中の位置"
                );
                workspace.open_slot_files(window, cx);
                assert_eq!(
                    workspace.tabs[workspace.active_tab].path, a,
                    "端末が {terminals} つあっても a に戻る"
                );
                workspace.sync_active_slot();
                assert_eq!(
                    workspace.active_slot().map(|slot| slot.active_file),
                    Some(0)
                );
            }
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui::test]
    fn terminals_open_as_editor_tabs_and_move_to_and_from_the_dock(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_terminal_tabs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("notes.txt"), "hello\n").unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        let notes = root.join("notes.txt");
        let (terminal, session) = workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace
                .terminal_dock
                .update(cx, |dock, _cx| dock.use_test_terminals());
            workspace.open_file_sync(notes.clone(), window, cx);
            workspace.new_terminal_tab(&NewTerminalTab, window, cx);
            assert_eq!(workspace.tabs.len(), 2, "ファイルのタブと並ぶ");
            assert_eq!(workspace.active_tab, 1);
            let (terminal, session) = workspace.tabs[1]
                .terminal()
                .map(|(view, session)| (view.clone(), session))
                .expect("端末のタブ");
            assert!(
                terminal.read(cx).focus_handle().is_focused(window),
                "開いた端末へ打鍵が行く"
            );
            assert_eq!(
                workspace
                    .terminal_dock
                    .read(cx)
                    .session(session)
                    .map(|view| view.entity_id()),
                Some(terminal.entity_id()),
                "PTY はドックが持つ"
            );
            assert_eq!(
                workspace.terminal_tab_label(1, cx),
                i18n::t!("terminal.tab_title", "n" => 1)
            );
            workspace.select_tab(0, window, cx);
            workspace.select_tab(1, window, cx);
            let slot = workspace.active_slot().expect("project");
            assert!(
                !slot
                    .explorer
                    .recent_files()
                    .contains(&terminal_tab_key(session)),
                "⌘P の最近に出さない"
            );
            assert_eq!(
                slot.open_files,
                vec![notes.clone()],
                "窓セッションに残さない"
            );
            (terminal, session)
        });

        // 端末の中の ⌘T = エディタ領域にもう 1 つ（下ドックには開かない）。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.dispatch_action(terminal_view::actions::NewTab);
        let second = workspace.update_in(cx, |workspace, _window, cx| {
            assert_eq!(workspace.tabs.len(), 3);
            assert!(!workspace.terminal_dock.read(cx).has_tabs());
            assert_eq!(
                workspace.terminal_tab_label(2, cx),
                i18n::t!("terminal.tab_title", "n" => 2)
            );
            workspace.tabs[2]
                .terminal()
                .map(|(_, id)| id)
                .expect("端末")
        });
        // ⌘\ は分割せずに案内する。⌘W は前にある端末のタブを閉じ、端末も終える。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.dispatch_action(terminal_view::actions::Split);
        cx.dispatch_action(terminal_view::actions::CloseTab);
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(
                workspace.tabs.len(),
                2,
                "⌘W で閉じる（何も動いていなければ聞かない）"
            );
            assert!(
                workspace.terminal_dock.read(cx).session(second).is_none(),
                "端末も終える"
            );
            assert!(workspace
                .notifications
                .toasts
                .iter()
                .any(|toast| toast.text.as_ref() == i18n::t!("terminal.tab_split_hint")));

            // 下ドックへ移す = 止めずにドックのタブへ。
            workspace.move_terminal_tab_to_dock(1, window, cx);
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.chrome.show_bottom);
            let dock = workspace.terminal_dock.read(cx);
            assert_eq!(
                dock.active_terminal().map(|view| view.entity_id()),
                Some(terminal.entity_id()),
                "同じ端末がドックへ"
            );
            assert!(dock.session(session).is_none());

            // ドックの前にいる端末をエディタ領域へ。最後の 1 つならドックは畳む。
            workspace.move_terminal_to_editor(&MoveTerminalToEditor, window, cx);
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(
                workspace.tabs[1]
                    .terminal()
                    .map(|(view, _)| view.entity_id()),
                Some(terminal.entity_id()),
                "同じ端末が戻る"
            );
            assert!(!workspace.terminal_dock.read(cx).has_tabs());
            assert!(!workspace.chrome.show_bottom);

            // ファイルのタブを開き直しても（ブランチの切り替え）端末のタブは残る。
            workspace.open_slot_files(window, cx);
            assert_eq!(workspace.tabs.len(), 2);
            assert!(workspace.tabs.iter().any(|tab| {
                tab.terminal()
                    .is_some_and(|(view, _)| view.entity_id() == terminal.entity_id())
            }));
            assert!(workspace.tabs.iter().any(|tab| tab.path == notes));
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
