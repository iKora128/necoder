//! ＋ Task の玄関（FLEET-V2 §4.1）。入力は 1 つ（複数行）で、1 行目が Task 名と `task/<slug>` になる。
//! 入力は EditorView を使い IME / ⌘⏎ を既存 composer と揃える。準備スクリプトの有無は開いた時に
//! 1 回だけ見る（render 中に Host I/O をしない規律）。
use crate::workspace::*;

/// 開いている ＋ Task ダイアログ。統合先（main の worktree）を基準に表示・作成する。
pub(crate) struct NewTaskDialog {
    editor: Entity<EditorView>,
    /// 「詳細 ▾」を開いているか（O20）。
    details_open: bool,
    /// ブランチ名（空 = `task/<slug>`・既にあるブランチならその worktree）。
    branch_editor: Entity<EditorView>,
    /// 新しいブランチの起点（空 = 統合先の HEAD）。
    base_editor: Entity<EditorView>,
    /// 統合先の worktree root。無ければ Task を切れない（ダイアログは案内だけ出す）。
    root: Option<PathBuf>,
    /// `.necoder/worktree-setup.sh` があるか（開いた時点・「作る」で true に）。
    setup_script_present: bool,
    /// 準備スクリプトを今回は流さない（詳細のチェック・O20）。`.worktreeinclude` は写す。
    skip_setup: bool,
    /// 開く直前にフォーカスがあった場所。取り消し（Esc / キャンセル）でそこへ返す。
    previous_focus: Option<FocusHandle>,
}

impl Workspace {
    /// アクティブなリポジトリの統合先 slot（`⌂ main`）。
    fn integration_index_for_active_repository(&self) -> Option<usize> {
        let key = self.active_repository_key()?;
        self.integration_slot_for(&key)
    }

    pub(super) fn open_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.new_task.is_some() { return; }
        let editor = cx.new(|cx| EditorView::plain(self.theme.clone(), self.accent(), false, cx));
        // 詳細の 2 欄は 1 行（⏎ で作る）。
        let branch_editor = cx.new(|cx| EditorView::plain(self.theme.clone(), self.accent(), true, cx));
        let base_editor = cx.new(|cx| EditorView::plain(self.theme.clone(), self.accent(), true, cx));
        for input in [&editor, &branch_editor, &base_editor] {
            cx.subscribe(input, |this, _, event, cx| {
                if matches!(event, ComposerEvent::Submit) { this.submit_new_task(cx); }
                cx.notify();
            }).detach();
        }
        let previous_focus = window.focused(cx);
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let integration = self.integration_index_for_active_repository().map(|index| &self.project_sessions.projects[index].worktree);
        let root = integration.map(|worktree| worktree.root().to_path_buf());
        let setup_script_present = integration.is_some_and(|worktree| {
            worktree.host().metadata(&project::worktree_setup_script(worktree.root())).is_ok()
        });
        self.chrome.new_task = Some(NewTaskDialog {
            editor,
            details_open: false,
            branch_editor,
            base_editor,
            root,
            setup_script_present,
            skip_setup: false,
            previous_focus,
        });
        cx.notify();
    }

    /// テスト用: ＋ Task の入力欄にフォーカスがあるか（ダイアログが無ければ false）。
    #[cfg(test)]
    pub(crate) fn new_task_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.chrome.new_task.as_ref().is_some_and(|dialog| dialog.editor.read(cx).focus_handle(cx).is_focused(window))
    }

    /// 取り消し（Esc / キャンセル）。開く前にいた場所へフォーカスを返す（返さないと
    /// focus-lost 復帰がエディタ等へ落とし、押す前に居たサイドバーから外れる）。
    /// テスト用: ＋ Task の入力欄の中身（ダイアログが無ければ None）。
    #[cfg(test)]
    pub(crate) fn new_task_input_text(&self, cx: &App) -> Option<String> {
        self.chrome.new_task.as_ref().map(|dialog| dialog.editor.read(cx).plain_text())
    }

    fn cancel_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.chrome.new_task.take() else { return; };
        if let Some(previous) = dialog.previous_focus {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn submit_new_task(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &self.chrome.new_task else { return; };
        if dialog.root.is_none() { return; }
        let prompt = dialog.editor.read(cx).plain_text();
        if prompt.trim().is_empty() { return; }
        let field = |input: &Entity<EditorView>| {
            let text = input.read(cx).plain_text().trim().to_string();
            (!text.is_empty()).then_some(text)
        };
        let start = project::TaskStart {
            branch: field(&dialog.branch_editor),
            base: field(&dialog.base_editor),
            skip_setup: dialog.skip_setup,
        };
        self.chrome.new_task = None;
        self.create_prompted_task(prompt, start, cx);
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
        let chosen_branch = dialog.branch_editor.read(cx).plain_text().trim().to_string();
        let chosen_base = dialog.base_editor.read(cx).plain_text().trim().to_string();
        let folder = if chosen_branch.is_empty() { slug.clone() } else { chosen_branch.replace('/', "-") };
        let worktree = dialog.root.as_deref().and_then(project::task_worktree_dir).map(|dir| dir.join(&folder));
        let branch_label = if chosen_branch.is_empty() { format!("task/{slug}") } else { chosen_branch.clone() };
        let branch_line = if chosen_base.is_empty() {
            format!("⎇ {branch_label}")
        } else {
            i18n::t!("fleet.new_task_from_base", "branch" => &branch_label, "base" => &chosen_base)
        };
        let mut body = div().id("new-task-dialog").debug_selector(|| "new-task-dialog".to_string()).w(px(600.)).max_w_full().p(px(20.)).flex().flex_col().gap(px(12.)).rounded(px(10.)).bg(self.theme.bg0).border_1().border_color(self.theme.border)
            .child(div().text_size(px(16.)).text_color(self.theme.fg0).child(i18n::t!("fleet.new_task")))
            .child(div().text_size(px(11.)).text_color(self.theme.fg2).child(i18n::t!("fleet.new_task_hint")))
            .child(div().h(px(150.)).border_1().border_color(self.theme.border).child(dialog.editor.clone()));
        if dialog.root.is_none() {
            body = body.child(div().text_size(px(11.)).text_color(self.theme.err).child(i18n::t!("fleet.new_task_no_repository")));
        } else {
            let setup_key = if dialog.setup_script_present { "fleet.new_task_setup_ready" } else { "fleet.new_task_setup_missing" };
            body = body
                .child(div().text_size(px(11.)).text_color(self.theme.fg1).child(SharedString::from(branch_line)))
                .child(div().text_size(px(11.)).text_color(self.theme.fg2).overflow_hidden().whitespace_nowrap()
                    .child(SharedString::from(i18n::t!("fleet.new_task_worktree", "path" => worktree.as_deref().map(|path| path.display().to_string()).unwrap_or_default()))))
                .child(div().flex().items_center().gap(px(10.)).text_size(px(11.))
                    .child(div().text_color(if dialog.setup_script_present { self.theme.fg1 } else { self.theme.fg2 }).child(SharedString::from(i18n::t!(setup_key))))
                    .when(!dialog.setup_script_present, |row| row.child(
                        div().id("new-task-setup-create").cursor_pointer().text_color(self.accent()).child(i18n::t!("fleet.new_task_setup_create"))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| { cx.stop_propagation(); this.create_setup_script(window, cx) })))));
            // 詳細 ▾（O20）: ブランチ名と起点。空のままなら従来どおり（task/<slug> を統合先の HEAD から）。
            let open = dialog.details_open;
            body = body.child(
                div().id("new-task-details").cursor_pointer().text_size(px(11.)).text_color(self.theme.fg2)
                    .hover(|style| style.text_color(self.theme.fg1))
                    .child(SharedString::from(format!("{} {}", if open { "▾" } else { "▸" }, i18n::t!("fleet.new_task_details"))))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(dialog) = this.chrome.new_task.as_mut() { dialog.details_open = !dialog.details_open; }
                        cx.notify();
                    })),
            );
            if open {
                let field = |label: String, input: &Entity<EditorView>| {
                    div().flex().flex_col().gap(px(3.))
                        .child(div().text_size(px(10.5)).text_color(self.theme.fg2).child(SharedString::from(label)))
                        // 押した欄にフォーカスを残す（ダイアログ全体の「主入力へ戻す」まで泡立たせない）。
                        .child(div().h(px(26.)).border_1().border_color(self.theme.border).child(input.clone())
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
                };
                body = body
                    .child(field(i18n::t!("fleet.new_task_branch_label"), &dialog.branch_editor))
                    .child(field(i18n::t!("fleet.new_task_base_label"), &dialog.base_editor));
                // 準備スクリプトを今回は流さない（O20・スクリプトがある時だけ）。
                if dialog.setup_script_present {
                    let skip = dialog.skip_setup;
                    body = body.child(
                        div().id("new-task-skip-setup").flex().items_center().gap(px(6.)).cursor_pointer()
                            .text_size(px(11.)).text_color(if skip { self.theme.fg0 } else { self.theme.fg1 })
                            .child(div().size(px(12.)).rounded(px(3.)).border_1().border_color(self.theme.fg2)
                                .flex().items_center().justify_center().text_size(px(9.))
                                .when(skip, |box_| box_.child("✓")))
                            .child(SharedString::from(i18n::t!("fleet.new_task_skip_setup")))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(dialog) = this.chrome.new_task.as_mut() { dialog.skip_setup = !dialog.skip_setup; }
                                cx.notify();
                            })),
                    );
                }
            }
        }
        body = body.child(div().flex().justify_end().gap(px(10.))
            .child(div().id("new-task-cancel").cursor_pointer().text_color(self.theme.fg2).child(i18n::t!("fleet.new_task_cancel"))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| { cx.stop_propagation(); this.cancel_new_task(window, cx); })))
            .child(div().id("new-task-submit").cursor_pointer().text_color(self.theme.fg0).child(i18n::t!("fleet.new_task_start"))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.submit_new_task(cx) }))));
        // 入力欄の外（余白・ラベル）を押しても入力欄へ戻す（ボタンは各自 stop_propagation で除外）。背面は `occlude` でクリックを通さない
        // ＝ダイアログ越しに裏のサイドバーや composer がフォーカスを取らない。
        let focus = dialog.editor.read(cx).focus_handle(cx);
        body = body
            .on_mouse_down(MouseButton::Left, move |_, window, cx| window.focus(&focus, cx))
            // Esc = 取り消し（UI-SPEC §7 のオーバーレイ共通）。入力欄は複数選択を畳む時以外
            // `editor::Cancel` を親へ流すので、ここで受ける。
            .on_action(cx.listener(|this, _: &editor_view::Cancel, window, cx| this.cancel_new_task(window, cx)));
        Some(div().absolute().inset_0().occlude().flex().items_center().justify_center().bg(gpui::rgba(0x00000088)).child(body).into_any_element())
    }
}
