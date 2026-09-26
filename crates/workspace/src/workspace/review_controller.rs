//! 変更レビューの結線。session ごとに 1 枚の `ReviewView` を、エディタのタブ（パレット
//! 「Git: 変更をレビュー」・ソース管理パネルのボタン）と Fleet の Task カードの「変更」タブに載せる。
//! Editor と Fleet は同時に描かないので、同じ Entity を両方の面で使い回す（開いた畳み・見た印が揃う）。
use crate::workspace::*;

/// 変更レビューのタブを見分けるパス（実在しない名前・タブ名は i18n で別に出す）。
const REVIEW_TAB_NAME: &str = "⇄ review";

impl Workspace {
    /// session の変更レビューに「どこを・何と比べるか」を渡し、まだなら読み込ませる。
    /// Task の base は台帳の復元で後から届くので、表示のたびに渡し直す。
    pub(crate) fn activate_review(
        &mut self,
        session_index: usize,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ReviewView>> {
        let slot = self.project_sessions.projects.get(session_index)?;
        let context = ReviewContext {
            host: slot.worktree.host().clone(),
            root: slot.worktree.root().to_path_buf(),
            task_base: (!slot.task_space.is_integration())
                .then(|| slot.task_space.base_oid.clone())
                .flatten(),
            accent: slot.color,
        };
        let review = self
            .project_sessions
            .sessions
            .get(session_index)?
            .review
            .clone()?;
        review.update(cx, |review, cx| {
            review.set_context(context, cx);
            review.activate(cx);
        });
        Some(review)
    }

    /// パレット「Git: 変更をレビュー」/ ソース管理パネルの「変更をレビュー」。
    /// Editor ではタブとして開き（既にあれば選ぶ）、Fleet では選択中の Task カードの「変更」タブへ。
    pub(crate) fn open_review_tab(
        &mut self,
        _: &OpenReview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chat_mode() {
            return;
        }
        let session_index = self.project_sessions.active;
        let Some(review) = self.activate_review(session_index, cx) else {
            return;
        };
        if self.chrome.fleet_mode {
            let space = self.project_sessions.projects[session_index]
                .task_space
                .id
                .clone();
            self.chrome
                .stage_tabs
                .insert(space.clone(), FleetPane::Diff { space });
            cx.notify();
            return;
        }
        self.exit_agent_full_screen(cx);
        self.chrome.show_settings = false;
        self.agent_active = false;
        if let Some(index) = self.tabs.iter().position(EditorTab::is_review) {
            self.select_tab(index, window, cx);
            return;
        }
        let Some(root) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.worktree.root().to_path_buf())
        else {
            return;
        };
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let handle = review.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.tabs.push(EditorTab {
            path: root.join(REVIEW_TAB_NAME),
            content: TabContent::Review(review),
            transient: true,
        });
        self.active_tab = self.tabs.len() - 1;
        cx.notify();
    }

    pub(crate) fn on_review_event(
        &mut self,
        review: Entity<ReviewView>,
        event: &ReviewEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(session_index) = self
            .project_sessions
            .sessions
            .iter()
            .position(|session| session.review.as_ref() == Some(&review))
        else {
            return;
        };
        match event {
            ReviewEvent::OpenFile { path, line } => {
                // Fleet の中にエディタは持たない（「ファイル」タブと同じ流儀）: Editor へ出て開く。
                self.chrome.fleet_mode = false;
                if session_index != self.project_sessions.active {
                    self.overlays.pending_project_switch = Some(session_index);
                }
                let row = line.map_or(0, |line| line.saturating_sub(1) as usize);
                self.project_sessions.sessions[session_index].pending_navigation =
                    Some((path.clone(), row, 0));
            }
        }
        cx.notify();
    }

    /// 作業ツリーが変わった（ファイル監視）: 開いている変更レビューに「新しい変更があります」を出す。
    pub(crate) fn mark_review_outdated(&mut self, session_index: usize, cx: &mut Context<Self>) {
        let review = self
            .project_sessions
            .sessions
            .get(session_index)
            .and_then(|session| session.review.clone());
        if let Some(review) = review {
            review.update(cx, |review, cx| review.mark_outdated(cx));
        }
    }
}
