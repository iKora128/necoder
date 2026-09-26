//! 端末のタブの改名（O24・C19）。タブをダブルクリックすると、スレッドの改名と同じ小さなダイアログ
//! （IME の効く平坦な EditorView）を出し、確定した名前をシェルが付けるタイトル（OSC 0 / 2）より先に
//! タブへ出す。空にして確定するとシェルのタイトルに戻る。名前は端末が開いている間だけ（保存しない）。
//!
//! ドックは terminal_view にあり、エディタを知らない（依存の向き）ので、入力欄はここで出す。
//! ドックのイベントは窓を持たないので、入力欄へのフォーカスは次の描画で渡す（`focus_next_frame`）。

use crate::workspace::*;

/// 改名中の端末と入力欄。
pub(crate) struct TerminalRenaming {
    pub(crate) terminal: Entity<TerminalView>,
    pub(crate) editor: Entity<EditorView>,
}

impl Workspace {
    pub(crate) fn start_terminal_rename(
        &mut self,
        terminal: Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        let current = terminal
            .read(cx)
            .display_title()
            .unwrap_or_default()
            .to_string();
        let accent = self.accent();
        let editor = cx.new(|cx| {
            let mut view = EditorView::plain(self.theme.clone(), accent, true, cx);
            view.set_plain_text(&current, cx);
            view
        });
        cx.subscribe(&editor, |workspace, _editor, event, cx| match event {
            ComposerEvent::Submit => workspace.confirm_terminal_rename(cx),
            // 改名入力は固定高（1 行）なので高さ追従は不要。
            ComposerEvent::ContentHeightChanged => {}
        })
        .detach();
        self.chrome.focus_next_frame = Some(editor.read(cx).focus_handle(cx));
        self.chrome.terminal_renaming = Some(TerminalRenaming { terminal, editor });
        cx.notify();
    }

    /// 確定（Enter・「変更」）。空・空白だけならシェルのタイトルに戻す。
    pub(crate) fn confirm_terminal_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.chrome.terminal_renaming.take() else {
            return;
        };
        let name = renaming.editor.read(cx).plain_text().trim().to_string();
        renaming.terminal.update(cx, |terminal, cx| {
            terminal.set_custom_title((!name.is_empty()).then_some(name), cx)
        });
        self.focus_terminal_after_rename(&renaming.terminal, cx);
        cx.notify();
    }

    /// 取り消し（Esc・「キャンセル」）。名前は変えない。
    pub(crate) fn cancel_terminal_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(renaming) = self.chrome.terminal_renaming.take() {
            self.focus_terminal_after_rename(&renaming.terminal, cx);
            cx.notify();
        }
    }

    /// 閉じたら改名した端末へ打鍵を戻す。
    fn focus_terminal_after_rename(
        &mut self,
        terminal: &Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        self.chrome.focus_next_frame = Some(terminal.read(cx).focus_handle());
    }

    /// 改名のダイアログ（スレッドの改名と同じ形・中央）。
    pub(crate) fn render_terminal_rename(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let renaming = self.chrome.terminal_renaming.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .px(px(13.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(if primary { accent } else { theme.border })
                .bg(if primary {
                    accent.alpha(0.16)
                } else {
                    theme.bg1
                })
                .text_size(px(12.))
                .text_color(if primary { accent } else { theme.fg1 })
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3))
                .child(SharedString::from(label))
        };
        let card = div()
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(px(10.))
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    this.cancel_terminal_rename(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _window, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!("terminal.rename_title"))),
            )
            .child(
                div()
                    .w_full()
                    .h(px(34.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(accent)
                    .bg(theme.bg0)
                    .overflow_hidden()
                    .child(renaming.editor.clone()),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("terminal.rename_hint"))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button(
                            "terminal-rename-cancel",
                            i18n::t!("terminal.rename_cancel"),
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.cancel_terminal_rename(cx)),
                        ),
                    )
                    .child(
                        button(
                            "terminal-rename-confirm",
                            i18n::t!("terminal.rename_confirm"),
                            true,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.confirm_terminal_rename(cx)),
                        ),
                    ),
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
                .bg(gpui::hsla(0., 0., 0., 0.25))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.cancel_terminal_rename(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn a_terminal_tab_takes_a_name_and_gives_it_back(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_terminal_rename_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
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
        // PTY を起こさない端末（テスト用）で、ドックのダブルクリックと同じ入口から改名する。
        let terminal = workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            let terminal = cx.new(|cx| TerminalView::new_test(Theme::dark(), cx));
            workspace.start_terminal_rename(terminal.clone(), cx);
            terminal
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        workspace.update_in(cx, |workspace, window, cx| {
            let renaming = workspace
                .chrome
                .terminal_renaming
                .as_ref()
                .expect("ダイアログ");
            assert!(
                renaming.editor.read(cx).focus_handle(cx).is_focused(window),
                "次の描画で入力欄へフォーカス"
            );
            let editor = renaming.editor.clone();
            editor.update(cx, |editor, cx| editor.set_plain_text("  dev server  ", cx));
            workspace.confirm_terminal_rename(cx);
        });
        terminal.read_with(cx, |terminal, _| {
            assert_eq!(terminal.display_title(), Some("dev server"));
        });

        // 空にして確定 = 名前を外す。取り消しは変えない。
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.start_terminal_rename(terminal.clone(), cx);
            let editor = workspace
                .chrome
                .terminal_renaming
                .as_ref()
                .unwrap()
                .editor
                .clone();
            editor.update(cx, |editor, cx| editor.set_plain_text("other", cx));
            workspace.cancel_terminal_rename(cx);
        });
        terminal.read_with(cx, |terminal, _| {
            assert_eq!(
                terminal.custom_title(),
                Some("dev server"),
                "取り消しは変えない"
            );
        });
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.start_terminal_rename(terminal.clone(), cx);
            let editor = workspace
                .chrome
                .terminal_renaming
                .as_ref()
                .unwrap()
                .editor
                .clone();
            editor.update(cx, |editor, cx| editor.set_plain_text("   ", cx));
            workspace.confirm_terminal_rename(cx);
        });
        terminal.read_with(cx, |terminal, _| {
            assert_eq!(terminal.custom_title(), None, "空はシェルのタイトルに戻す");
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
