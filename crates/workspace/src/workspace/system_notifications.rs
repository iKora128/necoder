//! OS のデスクトップ通知（O12）。
//!
//! 右下のトーストは necoder を見ている時にしか目に入らない。ターンの完了 / 失敗・承認待ち・
//! 質問待ちを、**その窓を見ていない時だけ**（別の窓・別のアプリ・隠している）通知センターへも出す。
//! 見ている時はトーストで足りるので出さない。スレッドのミュートと設定 `system_notifications` を守る。
//!
//! - 同じスレッドの通知は tag（スレッドの永続 id）で置き換える＝積み上がらない。
//! - 押すとアプリを前へ出してそのスレッドへ飛ぶ（トーストのクリックと同じ `jump_to_thread`）。
//!   行き先は tag → [`NotificationTarget`] の表で引く（通知には文字列しか載らない）。
//! - 実際に出すのは GPUI（mac は UNUserNotificationCenter）。**bundle ID の無い `cargo run` では
//!   GPUI が何もしない**（`.app` から起動した時だけ出る）。初回に OS の許可ダイアログが出る。

use crate::workspace::*;
use agent_panel::TurnOutcome;
use gpui::{SystemNotification, WeakEntity, WindowHandle};

/// 何を知らせるか（題の言い方が変わる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentAlert {
    Done,
    Interrupted,
    Failed,
    Permission,
    Question,
}

impl AgentAlert {
    pub(crate) fn from_outcome(outcome: TurnOutcome) -> Self {
        match outcome {
            TurnOutcome::Completed => AgentAlert::Done,
            TurnOutcome::Interrupted => AgentAlert::Interrupted,
            TurnOutcome::Failed => AgentAlert::Failed,
        }
    }

    fn title(self, thread: &str) -> String {
        match self {
            AgentAlert::Done => i18n::t!("notify.done", "thread" => thread),
            AgentAlert::Interrupted => i18n::t!("notify.interrupted", "thread" => thread),
            AgentAlert::Failed => i18n::t!("notify.failed", "thread" => thread),
            AgentAlert::Permission => i18n::t!("notify.permission", "thread" => thread),
            AgentAlert::Question => i18n::t!("notify.question", "thread" => thread),
        }
    }
}

/// OS 通知を出すか: 設定で有効・スレッドがミュートでない・**その窓を見ていない**
/// （窓が非アクティブ、またはアプリが隠れている / 前面に居ない）。
pub(crate) fn should_post_system_notification(
    enabled: bool,
    muted: bool,
    looking_at_window: bool,
) -> bool {
    enabled && !muted && !looking_at_window
}

/// 通知の tag（同じスレッドの通知を置き換える鍵）。
pub(crate) fn notification_tag(thread_id: &str) -> SharedString {
    SharedString::from(format!("necoder-thread-{thread_id}"))
}

/// 本文 = どこの（プロジェクト / Task / Chat）+ 何が。どちらかが空なら片方だけ。
fn notification_body(place: &str, detail: &str) -> String {
    match (place.trim().is_empty(), detail.trim().is_empty()) {
        (false, false) => format!("{place} — {detail}"),
        (false, true) => place.to_string(),
        (true, _) => detail.to_string(),
    }
}

/// 押された通知の行き先（窓・パネル・スレッドの永続 id）。
#[derive(Clone)]
struct NotificationTarget {
    window: WindowHandle<Workspace>,
    panel: WeakEntity<AgentPanel>,
    thread_id: String,
}

/// tag → 行き先。スレッドごとに 1 行（置き換え）なので、スレッドの数より増えない。
#[derive(Default)]
struct SystemNotificationTargets(HashMap<SharedString, NotificationTarget>);

impl gpui::Global for SystemNotificationTargets {}

/// アプリ全体の通知まわりを起動時に 1 回だけ繋ぐ（main から呼ぶ）: 通知を押した時の行き先と、
/// Dock バッジの数え直し（スレッドの状態台帳が変わるたび）。
pub fn install_agent_notifications(cx: &mut App) {
    cx.on_system_notification_response(|response, cx| open_notified_thread(&response.tag, cx));
    cx.observe_global::<agent_panel::RunningRegistry>(super::dock_badge::refresh_dock_badge)
        .detach();
}

/// 通知を押した: アプリを前へ出し、その窓のそのスレッドへ飛ぶ。窓やスレッドが既に無ければ
/// 前へ出すだけ。
fn open_notified_thread(tag: &SharedString, cx: &mut App) {
    cx.activate(true);
    let target = cx
        .try_global::<SystemNotificationTargets>()
        .and_then(|targets| targets.0.get(tag).cloned());
    let Some(target) = target else {
        return;
    };
    let opened = target.window.update(cx, |workspace, window, cx| {
        window.activate_window();
        workspace.jump_to_notified_thread(&target.panel, &target.thread_id, window, cx);
    });
    if let Err(error) = opened {
        eprintln!("通知の行き先の窓が無い: {error:#}");
    }
}

impl Workspace {
    /// いまこの窓を見ているか（アプリが前面で、この窓がキー窓）。隠している・他のアプリ・別の窓を
    /// 見ている時は false。
    pub(crate) fn is_looking_at_this_window(&self, cx: &App) -> bool {
        self.window_handle
            .is_some_and(|handle| cx.active_window() == Some(handle))
    }

    /// エージェントの出来事を OS の通知センターへ出す（出すかどうかは
    /// [`should_post_system_notification`]）。`place` = どこの（プロジェクト名 / Task 名 / Chat）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn post_agent_notification(
        &mut self,
        alert: AgentAlert,
        panel: &Entity<AgentPanel>,
        thread_id: &SharedString,
        thread: &SharedString,
        place: &str,
        detail: &str,
        muted: bool,
        cx: &mut Context<Self>,
    ) {
        // 通知の履歴（ベル・O13）には、見ているかどうかに関わらず残す（ミュートは残さない）。
        if !muted {
            self.record_inbox(
                alert.title(thread),
                notification_body(place, detail),
                panel,
                thread_id,
                cx,
            );
        }
        let enabled = settings::get(cx).system_notifications;
        if !should_post_system_notification(enabled, muted, self.is_looking_at_this_window(cx)) {
            return;
        }
        let Some(window) = self
            .window_handle
            .and_then(|handle| handle.downcast::<Workspace>())
        else {
            return;
        };
        let tag = notification_tag(thread_id);
        cx.default_global::<SystemNotificationTargets>().0.insert(
            tag.clone(),
            NotificationTarget {
                window,
                panel: panel.downgrade(),
                thread_id: thread_id.to_string(),
            },
        );
        cx.show_system_notification(SystemNotification {
            tag,
            title: SharedString::from(alert.title(thread)),
            body: SharedString::from(notification_body(place, detail)),
            actions: Vec::new(),
        });
    }

    /// 通知を押した先: Chat のスレッドなら Chat を開いてそのチャットへ、プロジェクトのスレッドなら
    /// トーストのクリックと同じく当該プロジェクト + スレッドへ（Fleet 中は Task カードへ）。
    pub(crate) fn jump_to_notified_thread(
        &mut self,
        panel: &WeakEntity<AgentPanel>,
        thread_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = panel.upgrade() else {
            return;
        };
        let Some(thread_index) = panel.read(cx).thread_position(thread_id) else {
            return; // スレッドが閉じられた
        };
        if self.chat_panel().as_ref() == Some(&panel) {
            self.set_chat_mode(true, window, cx);
            panel.update(cx, |panel, cx| panel.open_chat(thread_id, cx));
            return;
        }
        let Some(session_index) = self
            .project_sessions
            .sessions
            .iter()
            .position(|session| session.fleet_agents.contains(&panel))
        else {
            return;
        };
        // Fleet で足したエージェントは session の「いまの操作先」ではないことがある。
        self.project_sessions.sessions[session_index].agent_panel = panel;
        self.jump_to_thread(session_index, thread_index, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifies_only_when_nobody_is_looking() {
        assert!(should_post_system_notification(true, false, false));
        assert!(
            !should_post_system_notification(true, false, true),
            "その窓を見ている時はトーストで足りる"
        );
    }

    #[test]
    fn a_muted_thread_or_the_setting_turns_it_off() {
        assert!(
            !should_post_system_notification(true, true, false),
            "ミュート"
        );
        assert!(
            !should_post_system_notification(false, false, false),
            "設定で切った"
        );
    }

    #[test]
    fn one_thread_keeps_one_notification() {
        assert_eq!(notification_tag("t1"), notification_tag("t1"));
        assert_ne!(notification_tag("t1"), notification_tag("t2"));
    }

    #[test]
    fn the_body_names_the_place_then_the_detail() {
        assert_eq!(
            notification_body("necoder", "全部 green"),
            "necoder — 全部 green"
        );
        assert_eq!(notification_body("necoder", ""), "necoder");
        assert_eq!(notification_body("", "全部 green"), "全部 green");
    }
}
