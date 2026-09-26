//! statusbar の項目の出し入れ（O27）。statusbar を右クリックすると、項目ごとに ✓ の付いたメニューを出し、
//! 押すと settings.json の `statusbar_hidden` を書き換える（即座に効き、次の起動でも同じ）。
//!
//! 出し入れできるのは「見たい人と見たくない人がいる」項目だけ。知らせ（SSH の接続・承認待ち・
//! クラッシュ・更新）は出し入れの対象にしない（消すと気づけなくなる）。

use crate::workspace::*;

/// 出し入れできる項目（左から）。id は `statusbar_hidden` に書く名前、2 つ目はメニューの文言の i18n キー。
pub(crate) const STATUSBAR_ITEMS: [(&str, &str); 9] = [
    ("color", "statusbar.item_color"),
    ("branch", "statusbar.item_branch"),
    ("diagnostics", "statusbar.item_diagnostics"),
    ("terminal", "statusbar.item_terminal"),
    ("activity", "statusbar.item_activity"),
    ("usage", "statusbar.item_usage"),
    ("cursor", "statusbar.item_cursor"),
    ("encoding", "statusbar.item_encoding"),
    ("language", "statusbar.item_language"),
];

/// `id` の項目を出すか（`statusbar_hidden` に無ければ出す）。
pub(crate) fn statusbar_shows(hidden: &[String], id: &str) -> bool {
    !hidden.iter().any(|item| item == id)
}

/// `id` を出し入れした後の `statusbar_hidden`（隠していれば外し、出していれば足す）。
pub(crate) fn toggled_hidden(hidden: &[String], id: &str) -> Vec<String> {
    if statusbar_shows(hidden, id) {
        let mut next = hidden.to_vec();
        next.push(id.to_string());
        next
    } else {
        hidden.iter().filter(|item| *item != id).cloned().collect()
    }
}

impl Workspace {
    /// statusbar の右クリック: 押した所の上にメニューを出す。
    pub(crate) fn show_statusbar_menu(
        &mut self,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.chrome.statusbar_menu = Some(position);
        cx.notify();
    }

    pub(crate) fn hide_statusbar_menu(&mut self, cx: &mut Context<Self>) {
        if self.chrome.statusbar_menu.take().is_some() {
            cx.notify();
        }
    }

    /// 項目を出し入れする（settings.json へ書く・書けなければ知らせる）。メニューは開いたまま。
    pub(crate) fn toggle_statusbar_item(&mut self, id: &str, cx: &mut Context<Self>) {
        let next = toggled_hidden(&settings::get(cx).statusbar_hidden, id);
        if let Err(error) =
            settings::set_user_value(cx, "statusbar_hidden", serde_json::json!(next))
        {
            self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx);
        }
        cx.notify();
    }

    /// 項目のメニュー（右クリックした所の上・幅 200・✓ = 出している）。外側を押すと閉じる。
    pub(crate) fn render_statusbar_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let position = self.chrome.statusbar_menu?;
        let theme = self.theme.clone();
        let hidden = settings::get(cx).statusbar_hidden.clone();
        let row_height = 24.;
        let height = row_height * STATUSBAR_ITEMS.len() as f32 + 34.;
        let menu = div()
            .absolute()
            .left(position.x)
            .top((position.y - px(height)).max(px(8.)))
            .w(px(200.))
            .p(px(4.))
            .flex()
            .flex_col()
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            .text_size(px(12.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(9.))
                    .py(px(4.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("statusbar.menu_title"))),
            )
            .children(
                STATUSBAR_ITEMS
                    .iter()
                    .enumerate()
                    .map(|(index, (id, label))| {
                        let shown = statusbar_shows(&hidden, id);
                        let id = *id;
                        div()
                            .id(("statusbar-item", index))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(row_height))
                            .px(px(9.))
                            .rounded(px(5.))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(12.))
                                    .text_color(theme.fg0)
                                    .child(if shown { "✓" } else { "" }),
                            )
                            .child(SharedString::from(i18n::t!(label)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    this.toggle_statusbar_item(id, cx)
                                }),
                            )
                    }),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.hide_statusbar_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _window, cx| this.hide_statusbar_menu(cx)),
                )
                .child(menu)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_toggle_in_and_out_of_the_hidden_list() {
        let hidden = vec!["encoding".to_string()];
        assert!(statusbar_shows(&hidden, "branch"));
        assert!(!statusbar_shows(&hidden, "encoding"));
        assert_eq!(
            toggled_hidden(&hidden, "branch"),
            vec!["encoding".to_string(), "branch".to_string()]
        );
        assert!(toggled_hidden(&hidden, "encoding").is_empty());
    }

    #[gpui::test]
    fn the_statusbar_menu_writes_the_hidden_items(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_statusbar_items_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path.clone()), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.show_statusbar_menu(point(px(300.), px(700.)), cx);
            workspace.toggle_statusbar_item("encoding", cx);
            workspace.toggle_statusbar_item("language", cx);
            assert_eq!(
                settings::get(cx).statusbar_hidden,
                vec!["encoding".to_string(), "language".to_string()]
            );
            workspace.toggle_statusbar_item("encoding", cx);
            assert_eq!(
                settings::get(cx).statusbar_hidden,
                vec!["language".to_string()]
            );
            assert!(
                workspace.chrome.statusbar_menu.is_some(),
                "押してもメニューは開いたまま"
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let written = std::fs::read_to_string(&settings_path).unwrap();
        assert!(written.contains("statusbar_hidden"), "{written}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
