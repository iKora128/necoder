//! Workspace-independent Git panel state and typed shell events.

use editor_view::EditorView;
use gpui::{
    div, Context, Entity, EventEmitter, FocusHandle, IntoElement, Pixels, Point, Render,
    SharedString, Window,
};
use project::{DiffHunk, GitWorktree, GraphCommit, WorkingChange};
use std::path::PathBuf;

#[derive(Clone)]
pub struct BranchMenu {
    pub position: Point<Pixels>,
    pub current: Option<String>,
    pub branches: Vec<String>,
    pub worktrees: Vec<GitWorktree>,
}

#[derive(Clone, Default)]
pub struct RepositorySnapshot {
    pub changes: Vec<WorkingChange>,
    pub history: Vec<GraphCommit>,
    pub github_slug: Option<String>,
}

pub enum GitPanelEvent {
    RepositoryChanged,
    OpenDiff(PathBuf),
    OpenWorktree {
        path: PathBuf,
        branch: Option<String>,
    },
    Toast {
        message: SharedString,
    },
    StageHunk(DiffHunk),
}

/// 1 ProjectSession に属する Git UI state。
pub struct GitPanel {
    pub open: bool,
    /// コミットメッセージの入力欄（IME の正しい平坦 `EditorView`＝⌘V・undo・日本語入力が効く。
    /// Enter は改行、⌘⏎ でコミット）。
    pub message: Entity<EditorView>,
    /// 新しいブランチ名の入力欄（＋ を押している間だけ在る。⏎ で作成・Esc で取消）。
    pub branch_name: Option<Entity<EditorView>>,
    pub focus: FocusHandle,
    pub busy: bool,
    pub branch_menu: Option<BranchMenu>,
    snapshot: RepositorySnapshot,
}

impl GitPanel {
    /// `message` = コミットメッセージ欄（テーマ・色を知っている shell が作って渡す）。
    pub fn new(message: Entity<EditorView>, cx: &mut Context<Self>) -> Self {
        Self {
            open: false,
            message,
            branch_name: None,
            focus: cx.focus_handle(),
            busy: false,
            branch_menu: None,
            snapshot: RepositorySnapshot::default(),
        }
    }

    pub fn snapshot(&self) -> RepositorySnapshot {
        self.snapshot.clone()
    }

    pub fn set_snapshot(&mut self, snapshot: RepositorySnapshot, cx: &mut Context<Self>) {
        self.snapshot = snapshot;
        cx.notify();
    }

    pub fn clear_snapshot(&mut self, cx: &mut Context<Self>) {
        self.snapshot = RepositorySnapshot::default();
        cx.notify();
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if !open {
            self.branch_name = None;
        }
        cx.notify();
    }

    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.busy = busy;
        cx.notify();
    }
}

impl EventEmitter<GitPanelEvent> for GitPanel {}

impl Render for GitPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_snapshot_defaults_empty() {
        let snapshot = RepositorySnapshot::default();
        assert!(snapshot.changes.is_empty());
        assert!(snapshot.history.is_empty());
        assert!(snapshot.github_slug.is_none());
    }
}
