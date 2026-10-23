//! スマホ連携の常設入口と、一度だけ表示する用途の案内。
use crate::workspace::*;

impl Workspace {
    fn dismiss_phone_hint(&mut self, cx: &mut Context<Self>) {
        if settings::get(cx).phone_hint_seen {
            return;
        }
        let result = settings::set_user_value(cx, "phone_hint_seen", serde_json::Value::Bool(true));
        self.report_settings_save(result, cx);
        cx.notify();
    }

    pub(crate) fn open_phone_connection(&mut self, cx: &mut Context<Self>) {
        self.dismiss_phone_hint(cx);
        self.chrome.show_settings = true;
        self.exit_agent_full_screen(cx);
        self.chrome
            .settings_view
            .update(cx, |view, cx| view.show_remote_page(cx));
        cx.notify();
    }

    pub(crate) fn render_phone_hint(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let settings = settings::get(cx);
        if !settings.onboarded
            || settings.phone_hint_seen
            || self.chrome.fleet_mode
            || self.chat_mode()
            || self.chrome.show_settings
        {
            return None;
        }
        let theme = self.theme.clone();
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .px(px(10.))
                .py(px(5.))
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(12.))
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3))
                .child(SharedString::from(label))
        };
        Some(
            div()
                .absolute()
                .left(px(RAIL_WIDTH + 8.))
                .bottom(px(STATUSBAR_HEIGHT + 45.))
                .w(px(272.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .rounded(px(8.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_color(theme.fg0)
                        .child(SharedString::from(i18n::t!("settings.remote_hint"))),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            button(
                                "phone-hint-connect",
                                i18n::t!("settings.remote_hint_connect"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.open_phone_connection(cx)),
                            ),
                        )
                        .child(
                            button("phone-hint-close", i18n::t!("settings.remote_hint_close"))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.dismiss_phone_hint(cx)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn phone_entry_opens_management_in_every_mode_and_hint_stays_dismissed(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = std::env::temp_dir().join(format!(
            "necoder_phone_entry_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            serde_json::json!({"onboarded":true,"agent_prewarm":false,"chat":{"directory":root.join("chats")}}).to_string(),
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path.clone()), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(vec![root.clone()], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, _, cx| {
            workspace.chrome.fleet_mode = false;
            workspace.chrome.show_settings = false;
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
                session
                    .terminal_dock
                    .update(cx, |dock, _| dock.use_test_terminals());
            }
            cx.notify();
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _, cx| {
            assert!(
                workspace.render_phone_hint(cx).is_some(),
                "案内の表示条件: onboarded={} seen={} fleet={} chat={} settings={}",
                settings::get(cx).onboarded,
                settings::get(cx).phone_hint_seen,
                workspace.chrome.fleet_mode,
                workspace.chat_mode(),
                workspace.chrome.show_settings
            );
        });
        let close = cx.debug_bounds("phone-hint-close").expect("初回案内が出る");
        cx.simulate_mouse_down(close.center(), MouseButton::Left, gpui::Modifiers::none());
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _, cx| {
            assert!(settings::get(cx).phone_hint_seen);
            assert!(workspace.render_phone_hint(cx).is_none());
        });
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(settings_path).unwrap()).unwrap();
        assert_eq!(saved["phone_hint_seen"], true);
        for mode in ["editor", "fleet", "chat"] {
            workspace.update_in(cx, |workspace, window, cx| {
                workspace.chrome.show_settings = false;
                workspace.set_chat_mode(mode == "chat", window, cx);
                if let Some(session) = workspace.project_sessions.chat.as_ref() {
                    session
                        .terminal_dock
                        .update(cx, |dock, _| dock.use_test_terminals());
                }
                workspace.chrome.fleet_mode = mode == "fleet";
                cx.notify();
            });
            cx.run_until_parked();
            let phone = cx
                .debug_bounds("rail-phone")
                .expect("全モードにスマホアイコンがある");
            cx.simulate_mouse_down(phone.center(), MouseButton::Left, gpui::Modifiers::none());
            cx.run_until_parked();
            workspace.update_in(cx, |workspace, _, _| {
                assert!(workspace.chrome.show_settings)
            });
            assert!(
                cx.debug_bounds("remote-issue-qr").is_some(),
                "{mode} から端末管理を表示できる"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
