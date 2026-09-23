//! ＋ Task の玄関（FLEET-V2 §4.1）。入力は 1 つ（複数行）で、1 行目が Task 名と `task/<slug>` になる。
//! 入力は EditorView を使い IME / ⌘⏎ を既存 composer と揃える。準備スクリプトの有無は開いた時に
//! 1 回だけ見る（render 中に Host I/O をしない規律）。
use crate::workspace::*;

/// 開いている ＋ Task ダイアログ。統合先（main の worktree）を基準に表示・作成する。
pub(crate) struct NewTaskDialog {
    editor: Entity<EditorView>,
    /// 統合先の worktree root。無ければ Task を切れない（ダイアログは案内だけ出す）。
    root: Option<PathBuf>,
    /// `.necoder/worktree-setup.sh` があるか（開いた時点・「作る」で true に）。
    setup_script_present: bool,
}

impl Workspace {
    /// アクティブなリポジトリの統合先 slot（`⌂ main`）。
    fn integration_index_for_active_repository(&self) -> Option<usize> {
        let key = self.active_repository_key()?;
        self.project_sessions.projects.iter().position(|slot| slot.task_space.is_integration() && slot.repository_key() == key)
    }

    pub(super) fn open_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.new_task.is_some() { return; }
        let editor = cx.new(|cx| EditorView::plain(self.theme.clone(), self.accent(), false, cx));
        cx.subscribe(&editor, |this, _, event, cx| {
            if matches!(event, ComposerEvent::Submit) { this.submit_new_task(cx); }
            cx.notify();
        }).detach();
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let integration = self.integration_index_for_active_repository().map(|index| &self.project_sessions.projects[index].worktree);
        let root = integration.map(|worktree| worktree.root().to_path_buf());
        let setup_script_present = integration.is_some_and(|worktree| {
            worktree.host().metadata(&project::worktree_setup_script(worktree.root())).is_ok()
        });
        self.chrome.new_task = Some(NewTaskDialog { editor, root, setup_script_present });
        cx.notify();
    }

    /// テスト用: ＋ Task の入力欄にフォーカスがあるか（ダイアログが無ければ false）。
    #[cfg(test)]
    pub(crate) fn new_task_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.chrome.new_task.as_ref().is_some_and(|dialog| dialog.editor.read(cx).focus_handle(cx).is_focused(window))
    }

    fn submit_new_task(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &self.chrome.new_task else { return; };
        if dialog.root.is_none() { return; }
        let prompt = dialog.editor.read(cx).plain_text();
        if prompt.trim().is_empty() { return; }
        self.chrome.new_task = None;
        self.create_prompted_task(prompt, cx);
        cx.notify();
    }

    /// 「作る」: §6.2 のテンプレを統合先の `.necoder/worktree-setup.sh` に書き、solo のエディタで開く。
    /// 既にあれば書かない（`NotExists`）。開いた後は ⌘N で戻ってくる。
    fn create_setup_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.integration_index_for_active_repository() else { return; };
        let worktree = self.project_sessions.projects[index].worktree.clone();
        let path = project::worktree_setup_script(worktree.root());
        match worktree.host().write_file(&path, project::WORKTREE_SETUP_TEMPLATE.as_bytes(), host::WriteCondition::NotExists) {
            Ok(_) => {}
            Err(error) if worktree.host().metadata(&path).is_ok() => {
                eprintln!("準備スクリプトは既にある: {error:#}");
            }
            Err(error) => {
                let color = self.project_sessions.projects[index].color;
                self.push_toast(SharedString::from(format!("{error:#}")), color, cx);
                return;
            }
        }
        self.chrome.new_task = None;
        self.chrome.fleet_mode = false;
        self.switch_project(index, window, cx);
        self.open_file(path, window, cx);
        cx.notify();
    }

    pub(super) fn render_new_task_dialog(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let dialog = self.chrome.new_task.as_ref()?;
        let prompt = dialog.editor.read(cx).plain_text();
        let slug = project::task_slug(&prompt);
        let worktree = dialog.root.as_deref().and_then(project::task_worktree_dir).map(|dir| dir.join(&slug));
        let mut body = div().id("new-task-dialog").w(px(600.)).max_w_full().p(px(20.)).flex().flex_col().gap(px(12.)).rounded(px(10.)).bg(self.theme.bg0).border_1().border_color(self.theme.border)
            .child(div().text_size(px(16.)).text_color(self.theme.fg0).child(i18n::t!("fleet.new_task")))
            .child(div().text_size(px(11.)).text_color(self.theme.fg2).child(i18n::t!("fleet.new_task_hint")))
            .child(div().h(px(150.)).border_1().border_color(self.theme.border).child(dialog.editor.clone()));
        if dialog.root.is_none() {
            body = body.child(div().text_size(px(11.)).text_color(self.theme.err).child(i18n::t!("fleet.new_task_no_repository")));
        } else {
            let setup_key = if dialog.setup_script_present { "fleet.new_task_setup_ready" } else { "fleet.new_task_setup_missing" };
            body = body
                .child(div().text_size(px(11.)).text_color(self.theme.fg1).child(SharedString::from(format!("⎇ task/{slug}"))))
                .child(div().text_size(px(11.)).text_color(self.theme.fg2).overflow_hidden().whitespace_nowrap()
                    .child(SharedString::from(i18n::t!("fleet.new_task_worktree", "path" => worktree.as_deref().map(|path| path.display().to_string()).unwrap_or_default()))))
                .child(div().flex().items_center().gap(px(10.)).text_size(px(11.))
                    .child(div().text_color(if dialog.setup_script_present { self.theme.fg1 } else { self.theme.fg2 }).child(SharedString::from(i18n::t!(setup_key))))
                    .when(!dialog.setup_script_present, |row| row.child(
                        div().id("new-task-setup-create").cursor_pointer().text_color(self.accent()).child(i18n::t!("fleet.new_task_setup_create"))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| { cx.stop_propagation(); this.create_setup_script(window, cx) })))));
        }
        body = body.child(div().flex().justify_end().gap(px(10.))
            .child(div().id("new-task-cancel").cursor_pointer().text_color(self.theme.fg2).child(i18n::t!("fleet.new_task_cancel"))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.chrome.new_task = None; cx.notify(); })))
            .child(div().id("new-task-submit").cursor_pointer().text_color(self.theme.fg0).child(i18n::t!("fleet.new_task_start"))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.submit_new_task(cx) }))));
        // 入力欄の外（余白・ラベル）を押しても入力欄へ戻す（ボタンは各自 stop_propagation で除外）。背面は `occlude` でクリックを通さない
        // ＝ダイアログ越しに裏のサイドバーや composer がフォーカスを取らない。
        let focus = dialog.editor.read(cx).focus_handle(cx);
        body = body.on_mouse_down(MouseButton::Left, move |_, window, cx| window.focus(&focus, cx));
        Some(div().absolute().inset_0().occlude().flex().items_center().justify_center().bg(gpui::rgba(0x00000088)).child(body).into_any_element())
    }
}
