//! 通知の履歴（ベル・O13・H02）。
//!
//! エージェントの出来事（完了 / 中断 / 失敗 / 承認待ち / 質問待ち）は、トースト（数秒で消える）と
//! OS の通知（見ていない時だけ）で知らせる。どちらも後から見返せないので、この窓の分を**履歴**に残す。
//!
//! - titlebar のベル（未読があれば件数）→ 一覧（新しい順）。行を押すとそのスレッドへ飛び、既読にする。
//! - 行の右の ● / ○ で既読にする / 未読に戻す。見出しの「すべて既読」。
//! - 残すのは窓ごとに最新 [`INBOX_LIMIT`] 件で、起動している間だけ（再起動では消える）。
//! - ミュートしたスレッドの出来事は残さない（トースト・OS の通知と同じ）。
//!
//! 数え方の入口は OS の通知と同じ `post_agent_notification` 1 か所（プロジェクトも Chat も通る）。

use crate::workspace::*;
use gpui::WeakEntity;

/// 窓ごとに残す件数。
pub(crate) const INBOX_LIMIT: usize = 100;
const POPOVER_WIDTH: f32 = 380.;

/// 履歴の 1 件。
pub(crate) struct InboxItem {
    /// 何が（「完了: スレッド名」等・OS の通知の題と同じ）。
    pub(crate) title: SharedString,
    /// どこの・何を（「プロジェクト — 最後の発言」等・OS の通知の本文と同じ）。
    pub(crate) body: SharedString,
    /// スレッドの色（識別）。
    color: Hsla,
    at_ms: i64,
    pub(crate) unread: bool,
    panel: WeakEntity<AgentPanel>,
    thread_id: SharedString,
}

/// 一覧が開いている間の状態（Esc を受けるフォーカスと、閉じた後に返す先）。
pub(crate) struct InboxPopoverState {
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
}

/// 履歴に 1 件足す（古いものから落とす）。
pub(crate) fn push_inbox_item(items: &mut Vec<InboxItem>, item: InboxItem) {
    items.push(item);
    if items.len() > INBOX_LIMIT {
        let overflow = items.len() - INBOX_LIMIT;
        items.drain(..overflow);
    }
}

impl Workspace {
    /// エージェントの出来事を履歴に残す（`post_agent_notification` から・ミュートは呼ばない）。
    pub(crate) fn record_inbox(
        &mut self,
        title: String,
        body: String,
        panel: &Entity<AgentPanel>,
        thread_id: &SharedString,
        cx: &mut Context<Self>,
    ) {
        let color = panel
            .read(cx)
            .thread_color_by_id(thread_id)
            .unwrap_or(self.theme.fg2);
        push_inbox_item(
            &mut self.notifications.inbox,
            InboxItem {
                title: title.into(),
                body: body.into(),
                color,
                at_ms: agent_panel::now_unix_ms(),
                unread: true,
                panel: panel.downgrade(),
                thread_id: thread_id.clone(),
            },
        );
        cx.notify();
    }

    /// 未読の数（titlebar のベルの件数）。
    pub(crate) fn inbox_unread_count(&self) -> usize {
        self.notifications
            .inbox
            .iter()
            .filter(|item| item.unread)
            .count()
    }

    /// パレット「通知: 履歴を開く」/ ベル: 一覧を開く（開いていれば閉じる）。
    pub(crate) fn toggle_inbox(
        &mut self,
        _: &ShowInbox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlays.inbox.is_some() {
            self.close_inbox(window, cx);
            return;
        }
        let previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.overlays.inbox = Some(InboxPopoverState {
            focus,
            previous_focus,
        });
        cx.notify();
    }

    fn close_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.inbox.take() else {
            return;
        };
        if let Some(previous) = state.previous_focus {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    /// 行を押した: 既読にしてそのスレッドへ飛ぶ（スレッドが閉じられていれば既読にするだけ）。
    fn open_inbox_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.notifications.inbox.get_mut(index) else {
            return;
        };
        item.unread = false;
        let (panel, thread_id) = (item.panel.clone(), item.thread_id.clone());
        self.close_inbox(window, cx);
        self.jump_to_notified_thread(&panel, &thread_id, window, cx);
    }

    /// 行の ● / ○: 既読と未読を入れ替える。
    fn toggle_inbox_item_read(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(item) = self.notifications.inbox.get_mut(index) {
            item.unread = !item.unread;
            cx.notify();
        }
    }

    fn mark_inbox_read(&mut self, cx: &mut Context<Self>) {
        for item in &mut self.notifications.inbox {
            item.unread = false;
        }
        cx.notify();
    }

    fn on_inbox_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            self.close_inbox(window, cx);
        }
    }

    /// titlebar のベル。未読があれば件数を添える（中立の面・識別色は使わない）。
    pub(crate) fn render_inbox_bell(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let unread = self.inbox_unread_count();
        let tip = if unread > 0 {
            i18n::t!("inbox.bell_tip_unread", "count" => unread)
        } else {
            i18n::t!("inbox.bell_tip")
        };
        let open = self.overlays.inbox.is_some();
        self.rail_icon(
            "titlebar-inbox",
            "icons/bell.svg",
            tip,
            if unread > 0 || open {
                theme.fg0
            } else {
                theme.fg2
            },
        )
        .relative()
        .when(open, |bell| bell.bg(theme.bg2))
        .when(unread > 0, |bell| {
            bell.child(
                div()
                    .absolute()
                    .top(px(2.))
                    .right(px(0.))
                    .min_w(px(14.))
                    .h(px(14.))
                    .px(px(3.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .bg(theme.bg3)
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(9.))
                    .text_color(theme.fg0)
                    .child(SharedString::from(if unread > 99 {
                        "99+".to_string()
                    } else {
                        unread.to_string()
                    })),
            )
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation(); // titlebar のドラッグを起こさない
                this.toggle_inbox(&ShowInbox, window, cx);
            }),
        )
    }

    /// 履歴の一覧（titlebar の右下に吊るすポップオーバー）。
    pub(crate) fn render_inbox(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.inbox.as_ref()?;
        let theme = self.theme.clone();
        let unread = self.inbox_unread_count();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!("inbox.title"))),
            )
            .when(unread > 0, |header| {
                header.child(
                    div()
                        .id("inbox-mark-all-read")
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(10.5))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        .child(SharedString::from(i18n::t!("inbox.mark_all_read")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                cx.stop_propagation();
                                this.mark_inbox_read(cx);
                            }),
                        ),
                )
            });

        let mut list = div()
            .id("inbox-list")
            .flex()
            .flex_col()
            .max_h(px(440.))
            .overflow_y_scroll();
        if self.notifications.inbox.is_empty() {
            list = list.child(
                div()
                    .px(px(12.))
                    .py(px(16.))
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("inbox.empty"))),
            );
        }
        for (index, item) in self.notifications.inbox.iter().enumerate().rev() {
            let unread = item.unread;
            list = list.child(
                div()
                    .id(("inbox-item", index))
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(7.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    // スレッドの色（識別）。
                    .child(
                        div()
                            .flex_none()
                            .mt(px(4.))
                            .size(px(8.))
                            .rounded_full()
                            .bg(item.color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(if unread { theme.fg0 } else { theme.fg1 })
                                    .when(unread, |title| {
                                        title.font_weight(gpui::FontWeight::SEMIBOLD)
                                    })
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(item.title.clone()),
                            )
                            .when(!item.body.is_empty(), |text| {
                                text.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.fg2)
                                        .line_clamp(2)
                                        .child(item.body.clone()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(agent_panel::relative_time_label(item.at_ms)),
                    )
                    // 未読 ● / 既読 ○（押すと入れ替え）。色は中立。
                    .child(
                        div()
                            .id(("inbox-item-read", index))
                            .flex_none()
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .hover(|style| style.bg(theme.bg2))
                            .child(
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(theme.fg2)
                                    .when(unread, |dot| dot.bg(theme.fg1)),
                            )
                            .tooltip(Tooltip::text(
                                if unread {
                                    i18n::t!("inbox.mark_read")
                                } else {
                                    i18n::t!("inbox.mark_unread")
                                },
                                theme.clone(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.toggle_inbox_item_read(index, cx);
                                }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_inbox_item(index, window, cx);
                        }),
                    ),
            );
        }
        let footer = div()
            .px(px(12.))
            .py(px(6.))
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(10.))
            .text_color(theme.fg2)
            .child(SharedString::from(
                i18n::t!("inbox.note", "count" => INBOX_LIMIT),
            ));
        let card = div()
            .absolute()
            .top(px(TITLEBAR_HEIGHT + 4.))
            .right(px(8.))
            .w(px(POPOVER_WIDTH))
            .flex()
            .flex_col()
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .overflow_hidden()
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            .track_focus(&state.focus)
            .on_key_down(cx.listener(Self::on_inbox_key_down))
            // カード内クリックは背景の「閉じる」に伝播させない。
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(list)
            .child(footer);
        // 透明な背景（外側クリックで閉じる）。
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_inbox(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inbox_keeps_only_the_newest_items() {
        let mut items = Vec::new();
        for number in 0..(INBOX_LIMIT + 5) {
            push_inbox_item(
                &mut items,
                InboxItem {
                    title: SharedString::from(format!("完了: {number}")),
                    body: SharedString::default(),
                    color: gpui::hsla(0., 0., 0., 1.),
                    at_ms: number as i64,
                    unread: true,
                    panel: WeakEntity::new_invalid(),
                    thread_id: SharedString::default(),
                },
            );
        }
        assert_eq!(items.len(), INBOX_LIMIT);
        assert_eq!(items[0].title.as_ref(), "完了: 5", "古いものから落とす");
    }

    #[gpui::test]
    fn agent_alerts_stay_in_the_inbox_until_read(cx: &mut gpui::TestAppContext) {
        use super::super::system_notifications::AgentAlert;
        let root = std::env::temp_dir().join(format!("necoder_inbox_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("作業フォルダを作れる");
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false,"system_notifications":false}"#,
        )
        .expect("設定を書ける");
        cx.update(|cx| {
            i18n::set_locale("ja");
            settings::init(Some(settings_path), None, cx);
        });
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        let panel = workspace.read_with(cx, |workspace, _| workspace.agent_panel.clone());
        if panel.read_with(cx, |panel, _| panel.thread_id(0)).is_none() {
            panel.update(cx, |panel, cx| panel.new_thread(cx));
        }
        let thread_id = panel
            .read_with(cx, |panel, _| panel.thread_id(0))
            .expect("スレッドがある");
        let name = SharedString::from("parser");
        workspace.update_in(cx, |workspace, window, cx| {
            // OS の通知を切っていても、見ていても、履歴には残る。ミュートは残さない。
            workspace.post_agent_notification(
                AgentAlert::Done,
                &panel,
                &thread_id,
                &name,
                "necoder",
                "直した",
                false,
                cx,
            );
            workspace.post_agent_notification(
                AgentAlert::Permission,
                &panel,
                &thread_id,
                &name,
                "necoder",
                "Bash",
                true,
                cx,
            );
            assert_eq!(workspace.notifications.inbox.len(), 1);
            assert_eq!(workspace.inbox_unread_count(), 1);
            let item = &workspace.notifications.inbox[0];
            assert!(item.title.contains("parser"), "{}", item.title);
            assert_eq!(item.body.as_ref(), "necoder — 直した");

            workspace.toggle_inbox(&ShowInbox, window, cx);
            assert!(workspace.overlays.inbox.is_some());
            // 押す = 既読にして閉じ、そのスレッドへ。
            workspace.open_inbox_item(0, window, cx);
            assert_eq!(workspace.inbox_unread_count(), 0);
            assert!(workspace.overlays.inbox.is_none());
            // 未読に戻す / すべて既読。
            workspace.toggle_inbox_item_read(0, cx);
            assert_eq!(workspace.inbox_unread_count(), 1);
            workspace.mark_inbox_read(cx);
            assert_eq!(workspace.inbox_unread_count(), 0);
        });
        std::fs::remove_dir_all(&root).ok();
    }
}
