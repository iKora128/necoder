//! 確認待ち（UI-SPEC §8）: 件数の数え方と、赤い丸・白い点・statusbar の集計。
//!
//! バッジも集計も Dock も、スレッドの状態（`AgentPanel::attention_counts`）から同じ数え方で導く。
//! Editor でも Fleet でも同じものを出す。

use crate::workspace::*;
use agent_panel::AttentionCounts;

/// 赤い丸の数字の色。白は `err` の上で読みにくい（3.2:1）ので、どのテーマでも濃色に固定する。
const BADGE_TEXT: u32 = 0x16181e;

impl Workspace {
    /// レールの 1 枠（ワークスペース）の件数。Failed な Task と裁いていない分解案は、スレッドの
    /// 状態に関わらずあなた待ちに数える。アーカイブした Task は数えない（一覧にも出ない）。
    pub(crate) fn slot_attention(&self, index: usize, cx: &App) -> AttentionCounts {
        let (Some(slot), Some(session)) = (
            self.project_sessions.projects.get(index),
            self.project_sessions.sessions.get(index),
        ) else {
            return AttentionCounts::default();
        };
        let is_task = !slot.task_space.is_integration();
        if is_task && slot.task_space.phase == TaskPhase::Archived {
            return AttentionCounts::default();
        }
        let mut counts = AttentionCounts::default();
        for panel in &session.fleet_agents {
            counts += panel.read(cx).attention_counts();
        }
        if is_task && slot.task_space.phase == TaskPhase::Failed {
            counts.waiting += 1;
            counts.unread = 0; // 同じ事象を未確認と二重に数えない
        }
        if self.integration_slot_for(slot.repository_key()) == Some(index) {
            counts.waiting += self.captain_proposals_for(slot.repository_key()).count();
        }
        counts
    }

    /// 同じリポジトリの件数の合計（Task の worktree はレールに載らないので親の枠へ足す）。
    pub(crate) fn repository_attention(&self, repository: &str, cx: &App) -> AttentionCounts {
        let mut counts = AttentionCounts::default();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if slot.repository_key() == repository {
                counts += self.slot_attention(index, cx);
            }
        }
        counts
    }

    /// この窓の全体の件数（全ワークスペース + Chat）。
    pub(crate) fn attention_totals(&self, cx: &App) -> AttentionCounts {
        let mut counts = AttentionCounts::default();
        for index in 0..self.project_sessions.projects.len() {
            counts += self.slot_attention(index, cx);
        }
        if let Some(panel) = self.chat_panel() {
            counts += panel.read(cx).attention_counts();
        }
        counts
    }

    /// 赤い丸（あなた待ちの件数）。高さ 16px・10px 太字。
    pub(crate) fn attention_badge(&self, count: usize) -> Div {
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(16.))
            .h(px(16.))
            .px(px(4.))
            .rounded(px(8.))
            .bg(self.theme.err)
            .text_size(px(10.))
            .line_height(px(16.))
            .font_weight(FontWeight::BOLD)
            .text_color(gpui::rgb(BADGE_TEXT))
            .child(SharedString::from(badge_label(count)))
    }

    /// Box アイコンの印: あなた待ちがあれば赤い丸、無くて未確認があれば白い丸に本数を出す。
    pub(crate) fn attention_count_mark(&self, counts: AttentionCounts) -> Option<gpui::AnyElement> {
        if counts.waiting > 0 {
            return self.attention_mark(counts);
        }
        (counts.unread > 0).then(|| {
            div()
                .absolute()
                .top(px(-7.))
                .right(px(-8.))
                .flex()
                .items_center()
                .justify_center()
                .min_w(px(20.))
                .h(px(20.))
                .px(px(4.))
                .rounded(px(10.))
                .bg(self.theme.fg0)
                .border_2()
                .border_color(self.theme.bg0)
                .text_size(px(10.))
                .line_height(px(16.))
                .font_weight(FontWeight::BOLD)
                .text_color(self.theme.bg0)
                .child(SharedString::from(badge_label(counts.unread)))
                .into_any_element()
        })
    }

    /// アイコンの右上の印: あなた待ちがあれば赤い丸、無くて未確認があれば白い点、どちらも無ければ
    /// 何も出さない。面と同じ色の 2px で切り抜いて、アイコンから浮かせる。親は `.relative()`。
    pub(crate) fn attention_mark(&self, counts: AttentionCounts) -> Option<gpui::AnyElement> {
        let surface = self.theme.bg0;
        if counts.waiting > 0 {
            return Some(
                self.attention_badge(counts.waiting)
                    .absolute()
                    .top(px(-7.))
                    .right(px(-8.))
                    .min_w(px(20.))
                    .h(px(20.))
                    .rounded(px(10.))
                    .border_2()
                    .border_color(surface)
                    .into_any_element(),
            );
        }
        (counts.unread > 0).then(|| {
            div()
                .absolute()
                .top(px(-3.))
                .right(px(-3.))
                .size(px(12.))
                .rounded_full()
                .bg(self.theme.fg0)
                .border_2()
                .border_color(surface)
                .into_any_element()
        })
    }

    /// statusbar の中央の集計: `[2] あなた待ち · 未確認 2 · 生成中 1`。0 の項目は省き、全部 0 なら
    /// 何も出さない。押す = 次の確認待ちへ（⌘⇧U と同じ）。
    pub(crate) fn render_attention_summary(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let counts = self.attention_totals(cx);
        let container = div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .h_full();
        if counts == AttentionCounts::default() {
            return container.into_any_element();
        }
        let separator = || {
            div()
                .text_color(theme.fg2)
                .px(px(2.))
                .child(SharedString::from("·"))
        };
        let mut summary = div()
            .id("statusbar-attention")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(20.))
            .px(px(8.))
            .rounded(px(4.))
            .text_size(px(10.5))
            .text_color(theme.fg1)
            .whitespace_nowrap();
        let mut first = true;
        if counts.waiting > 0 {
            summary = summary.child(self.attention_badge(counts.waiting)).child(
                div()
                    .text_color(theme.fg0)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(i18n::t!("attention.waiting"))),
            );
            first = false;
        }
        if counts.unread > 0 {
            if !first {
                summary = summary.child(separator());
            }
            summary = summary.child(SharedString::from(
                i18n::t!("attention.unread", "n" => counts.unread),
            ));
            first = false;
        }
        if counts.working > 0 {
            if !first {
                summary = summary.child(separator());
            }
            summary = summary.child(SharedString::from(
                i18n::t!("attention.working", "n" => counts.working),
            ));
        }
        let clickable = counts.to_check() > 0;
        container
            .child(summary.when(clickable, |summary| {
                summary
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
                    .tooltip(Tooltip::text(i18n::t!("attention.next_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.control_next(&ControlNext, window, cx);
                        }),
                    )
            }))
            .into_any_element()
    }
}

/// 2 桁を超えたら詰める（丸の幅を保つ）。
pub(crate) fn badge_label(count: usize) -> String {
    if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_workspace<'a>(
        cx: &'a mut gpui::TestAppContext,
        name: &str,
    ) -> (
        Entity<Workspace>,
        &'a mut gpui::VisualTestContext,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!("necoder_{name}_{}", std::process::id()));
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
        let (workspace, vcx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        (workspace, vcx, root)
    }

    /// Editor でも Fleet でも同じ数え方。Failed な Task は未確認と二重に数えない。
    #[gpui::test]
    fn counts_are_the_same_in_editor_and_fleet(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = open_workspace(cx, "attention_counts");
        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(workspace.attention_totals(cx), AttentionCounts::default());
            let panel = workspace.agent_panel.clone();
            panel.update(cx, |panel, cx| panel.debug_ask_question(cx));
            let editor = workspace.attention_totals(cx);
            assert_eq!(editor.waiting, 1);
            assert_eq!(workspace.attention_badge_count(cx), 1, "Dock も同じ数");
            let repository = workspace.project_sessions.projects[0]
                .repository_key()
                .to_string();
            assert_eq!(workspace.repository_attention(&repository, cx), editor);

            workspace.toggle_fleet_mode(&ToggleFleet, window, cx);
            assert_eq!(workspace.attention_totals(cx), editor, "Fleet でも同じ");
        });
        std::fs::remove_dir_all(&root).ok();
    }

    /// ⌘⇧U は Editor でも効き、今見ているものの次へ進む。
    #[gpui::test]
    fn next_goes_to_the_thread_waiting_from_the_editor(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = open_workspace(cx, "attention_next");
        let panel = workspace.read_with(cx, |workspace, _| workspace.agent_panel.clone());
        panel.update(cx, |panel, cx| {
            panel.new_thread(cx);
            panel.new_thread(cx);
            panel.focus_thread(1, cx);
            panel.debug_ask_question(cx);
            panel.focus_thread(0, cx);
        });
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(!workspace.chrome.fleet_mode);
            workspace.control_next(&ControlNext, window, cx);
        });
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.active_thread()),
            1,
            "あなた待ちのスレッドへ飛ぶ"
        );
        // 何度押しても、唯一の確認待ちから動かない（答えるまで残る）。
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.control_next(&ControlNext, window, cx);
        });
        assert_eq!(panel.read_with(cx, |panel, _| panel.active_thread()), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn badge_label_caps_at_two_digits() {
        assert_eq!(badge_label(1), "1");
        assert_eq!(badge_label(99), "99");
        assert_eq!(badge_label(100), "99+");
    }

    #[test]
    fn counts_split_each_activity_and_only_check_what_needs_a_person() {
        use agent_panel::ThreadActivity;
        let mut counts = AttentionCounts::default();
        for activity in [
            ThreadActivity::Blocked,
            ThreadActivity::Blocked,
            ThreadActivity::Done { interrupted: false },
            ThreadActivity::Done { interrupted: true },
            ThreadActivity::Done { interrupted: false },
            ThreadActivity::Working,
            ThreadActivity::Idle,
        ] {
            counts.add_activity(activity);
        }
        assert_eq!(
            counts,
            AttentionCounts {
                waiting: 2,
                unread: 3,
                working: 1
            }
        );
        assert_eq!(counts.to_check(), 5, "生成中と確認済みは確認待ちに入らない");
    }
}
