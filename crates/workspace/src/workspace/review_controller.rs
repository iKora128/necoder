//! 変更レビューの結線。session ごとに 1 枚の `ReviewView` を、エディタのタブ（パレット
//! 「Git: 変更をレビュー」・ソース管理パネルのボタン）と Fleet の Task カードの「変更」タブに載せる。
//! Editor と Fleet は同時に描かないので、同じ Entity を両方の面で使い回す（開いた畳み・見た印が揃う）。
//!
//! 注記（行コメント）の宛先の一覧と送信はここが持つ（ビューは AI を知らない）。送信は
//! `AgentPanel::send_user_prompt_to` = 人間の発話として・表示中のタブを奪わず・実行中なら送信待ちへ。
use crate::workspace::*;
use review_view::SendTarget;

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
            storage: self.persistence.storage.clone(),
            // 注記を束ねる単位: Fleet = Task / Editor = プロジェクト（どちらも TaskSpace の id）。
            scope: slot.task_space.id.0.clone(),
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
            ReviewEvent::SendMenuRequested { resend } => {
                let targets = self.review_send_targets(session_index, cx);
                let resend = *resend;
                review.update(cx, |review, cx| review.show_send_menu(targets, resend, cx));
            }
            ReviewEvent::SendNotes {
                target,
                prompt,
                note_ids,
            } => self.send_review_notes(session_index, &review, target, prompt, note_ids, cx),
        }
        cx.notify();
    }

    /// 注記の宛先の一覧。既定（Fleet = Task カードの操作先 / Editor = アクティブなスレッド）を先頭に、
    /// session の全スレッドと「新しいスレッドで送る」を並べる。
    fn review_send_targets(&self, session_index: usize, cx: &App) -> Vec<SendTarget> {
        let Some(session) = self.project_sessions.sessions.get(session_index) else {
            return Vec::new();
        };
        let mut targets = Vec::new();
        for (panel_index, panel) in session.fleet_agents.iter().enumerate() {
            let current = *panel == session.agent_panel;
            let panel = panel.read(cx);
            let active = panel.active_thread();
            for (thread_index, status) in panel.statuses().into_iter().enumerate() {
                targets.push(SendTarget {
                    panel: panel_index,
                    thread: Some(thread_index),
                    label: status.name.clone(),
                    detail: SharedString::from(format!(
                        "{} · {}",
                        status.agent,
                        activity_label(status.activity)
                    )),
                    color: status.color,
                    is_default: current && thread_index == active,
                });
            }
        }
        targets.sort_by_key(|target| !target.is_default);
        if let Some(panel) = session
            .fleet_agents
            .iter()
            .position(|panel| *panel == session.agent_panel)
            .or((!session.fleet_agents.is_empty()).then_some(0))
        {
            targets.push(SendTarget {
                panel,
                thread: None,
                label: SharedString::from(i18n::t!("review.send_new_thread")),
                detail: SharedString::default(),
                color: self.theme.fg2,
                is_default: targets.is_empty(),
            });
        }
        targets
    }

    /// 注記を 1 通のプロンプトにまとめて送る（人間の発話として・表示中のタブを奪わない）。
    fn send_review_notes(
        &mut self,
        session_index: usize,
        review: &Entity<ReviewView>,
        target: &SendTarget,
        prompt: &str,
        note_ids: &[String],
        cx: &mut Context<Self>,
    ) {
        let panel = self
            .project_sessions
            .sessions
            .get(session_index)
            .and_then(|session| session.fleet_agents.get(target.panel).cloned());
        let delivered = panel.and_then(|panel| {
            let thread = match target.thread {
                Some(thread) => thread,
                None => panel.update(cx, |panel, cx| panel.new_thread_index(cx)),
            };
            let sent = panel.update(cx, |panel, cx| {
                panel.send_user_prompt_to(thread, prompt.to_string(), cx)
            });
            let status = panel.read(cx).statuses().into_iter().nth(thread);
            sent.then_some(status).flatten()
        });
        match delivered {
            Some(status) => {
                review.update(cx, |review, cx| review.mark_notes_sent(note_ids, cx));
                let message = i18n::t!(
                    "review.sent_toast",
                    "count" => note_ids.len(),
                    "thread" => status.name
                );
                self.push_toast(message.into(), status.color, cx);
            }
            None => {
                let color = self
                    .project_sessions
                    .projects
                    .get(session_index)
                    .map(|slot| slot.color)
                    .unwrap_or_else(|| project_color(0));
                self.push_toast(i18n::t!("review.send_failed").into(), color, cx);
            }
        }
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
