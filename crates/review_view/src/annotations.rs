//! 変更レビューの行コメント（注記）: 行の選択・入力欄・注記のカード・トレイ・送り先メニュー。
//!
//! 行にホバーすると左端に ＋、押すか選んだ行で `c` → その行の下に入力欄（`EditorView` の平坦モード =
//! IME 安全・⌘⏎ で保存・Esc で取り消し）。⇧クリックかドラッグで複数行。注記は storage に残し
//! （再起動しても消えない）、下端のトレイから 1 通のプロンプトに束ねて送る。宛先の一覧と送信は
//! Workspace が持つ（このビューは AI を知らない）。
//!
//! 参考: stablyai/orca@646e9a5 の `docs/site/content/docs/review/annotate-ai-diff.mdx`（＋ / `c` で
//! 行に注記・まとめて送る・Resolve と未解決の再送）。機能の比較だけで、コードは写していない。

use crate::*;
use gpui::{
    div, prelude::*, px, Context, FontWeight, KeyDownEvent, ListOffset, MouseButton, SharedString,
    Window,
};

impl ReviewView {
    /// 束ねる単位が変わったら注記を読み直す（背景で読み、届いたら行へ差し込む）。
    pub(crate) fn load_notes_if_needed(&mut self, cx: &mut Context<Self>) {
        let Some(context) = self.context.as_ref() else {
            return;
        };
        if self.notes_scope.as_deref() == Some(context.scope.as_str()) {
            return;
        }
        let scope = context.scope.clone();
        self.notes_scope = Some(scope.clone());
        self.notes.clear();
        self.draft = None;
        let Some(storage) = context.storage.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let loading_scope = scope.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move { storage.load_review_notes(&loading_scope) })
                .await;
            let applied = this.update(cx, |this, cx| {
                if this.notes_scope.as_deref() != Some(scope.as_str()) {
                    return;
                }
                match loaded {
                    Ok(records) => {
                        for record in records {
                            // 読み終える前に作った注記は既に居る（二重に積まない）。
                            if this.notes.iter().any(|note| note.id == record.id) {
                                continue;
                            }
                            match ReviewNote::from_record(&record) {
                                Ok(note) => this.notes.push(note),
                                Err(error) => {
                                    eprintln!("注記を読めない（{}）: {error}", record.id)
                                }
                            }
                        }
                        this.notes.sort_by_key(|note| note.created_at);
                    }
                    Err(error) => eprintln!("注記を読めない: {error:#}"),
                }
                this.rebuild_rows();
                cx.notify();
            });
            if let Err(error) = applied {
                eprintln!("注記の反映先が無い: {error:#}");
            }
        })
        .detach();
    }

    /// 注記を付けられる行か（diff の行と、畳みを開いた文脈行）。
    pub(crate) fn is_line_row(&self, index: usize) -> bool {
        matches!(
            self.rows.get(index),
            Some(Row::Line { .. } | Row::Context { .. })
        )
    }

    /// 行を選ぶ（`extend` = ⇧クリック / ドラッグで同じファイルの中の範囲に伸ばす）。
    pub(crate) fn select_row(&mut self, index: usize, extend: bool, cx: &mut Context<Self>) {
        if !self.is_line_row(index) {
            return;
        }
        let same_file = |this: &Self, other: usize| {
            this.rows.get(other).map(Row::file) == this.rows.get(index).map(Row::file)
        };
        self.selection = match self.selection {
            Some(selection) if extend && same_file(self, selection.anchor) => Some(LineSelection {
                anchor: selection.anchor,
                head: index,
            }),
            _ => Some(LineSelection {
                anchor: index,
                head: index,
            }),
        };
        cx.notify();
    }

    /// ↑↓ で選択を動かす（⇧で範囲を伸ばす）。選択が無ければ画面の先頭の行から。
    pub(crate) fn move_selection(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let start = match self.selection {
            Some(selection) => selection.head,
            None => {
                let top = self.list.logical_scroll_top().item_ix;
                if let Some(first) = (top..self.rows.len()).find(|index| self.is_line_row(*index)) {
                    self.select_row(first, false, cx);
                    self.list.scroll_to_reveal_item(first);
                }
                return;
            }
        };
        let mut index = start as isize + delta;
        while index >= 0 && (index as usize) < self.rows.len() {
            if self.is_line_row(index as usize) {
                let target = index as usize;
                let crosses = self.rows.get(target).map(Row::file)
                    != self
                        .selection
                        .and_then(|selection| self.rows.get(selection.anchor))
                        .map(Row::file);
                self.select_row(target, extend && !crosses, cx);
                self.list.scroll_to_reveal_item(target);
                return;
            }
            index += delta;
        }
    }

    /// 選んだ行から注記の対象を作る。追加行・文脈行を含めば新しい側、削除行だけなら古い側。
    pub(crate) fn selection_target(&self) -> Option<DiffLinesTarget> {
        let selection = self.selection?;
        let diff = self.diff.as_ref()?;
        let (low, high) = if selection.anchor <= selection.head {
            (selection.anchor, selection.head)
        } else {
            (selection.head, selection.anchor)
        };
        let file = self.rows.get(low)?.file();
        let entry = diff.files.get(file)?;
        let syntax = self.syntax.get(file).cloned().flatten();
        let mut excerpt = Vec::new();
        let (mut new_lines, mut old_lines) = (Vec::new(), Vec::new());
        for row in self.rows.get(low..=high)? {
            match row {
                Row::Line {
                    file: row_file,
                    hunk,
                    line,
                } if *row_file == file => {
                    let line = entry.hunks.get(*hunk)?.lines.get(*line)?;
                    let kind = match line.kind {
                        DiffLineKind::Added => ExcerptKind::Added,
                        DiffLineKind::Removed => ExcerptKind::Removed,
                        DiffLineKind::Context => ExcerptKind::Context,
                    };
                    match line.kind {
                        DiffLineKind::Removed => old_lines.extend(line.old_line),
                        _ => new_lines.extend(line.new_line),
                    }
                    excerpt.push(ExcerptLine {
                        kind,
                        text: line.text.clone(),
                    });
                }
                Row::Context {
                    file: row_file,
                    new_line,
                    ..
                } if *row_file == file => {
                    let text = syntax
                        .as_ref()
                        .and_then(|syntax| syntax.new.as_ref())
                        .and_then(|text| text.line(*new_line))
                        .map(|(text, _)| text.to_string())
                        .unwrap_or_default();
                    new_lines.push(*new_line);
                    excerpt.push(ExcerptLine {
                        kind: ExcerptKind::Context,
                        text,
                    });
                }
                _ => {}
            }
        }
        let (side, lines) = if new_lines.is_empty() {
            (NoteSide::Old, old_lines)
        } else {
            (NoteSide::New, new_lines)
        };
        Some(DiffLinesTarget {
            path: entry.path.clone(),
            side,
            start: *lines.iter().min()?,
            end: *lines.iter().max()?,
            base: diff.base_oid.clone(),
            excerpt,
        })
    }

    /// 選んだ行の下に注記の入力欄を開く（＋ / `c`）。
    pub fn open_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.selection_target() else {
            return;
        };
        self.start_draft(target, None, String::new(), window, cx);
    }

    pub(crate) fn start_draft(
        &mut self,
        target: DiffLinesTarget,
        editing: Option<String>,
        body: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme.clone();
        let accent = self.accent;
        let editor = cx.new(|cx| {
            let mut editor = EditorView::plain(theme, accent, false, cx);
            editor.set_plain_text(&body, cx);
            editor
        });
        let focus = editor.read(cx).focus_handle(cx);
        self.draft = Some(NoteDraft {
            target,
            editor,
            editing,
        });
        self.send_menu = None;
        self.rebuild_rows();
        if let Some(row) = self
            .rows
            .iter()
            .position(|row| matches!(row, Row::Draft { .. }))
        {
            self.list.scroll_to_reveal_item(row);
        }
        window.focus(&focus, cx);
        cx.notify();
    }

    /// 書きかけの本文を差し替える（プローブ・撮影用）。
    pub fn set_draft_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(draft) = &self.draft {
            draft
                .editor
                .update(cx, |editor, cx| editor.set_plain_text(text, cx));
        }
    }

    /// 注記を保存する（⌘⏎）。空なら何も残さず閉じる。直した注記は未送信に戻る。
    pub fn save_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        let body = draft.editor.read(cx).plain_text().trim().to_string();
        let now = now_ms();
        if !body.is_empty() {
            match draft.editing {
                Some(id) => {
                    if let Some(note) = self.notes.iter_mut().find(|note| note.id == id) {
                        note.body = body;
                        note.state = ReviewNoteState::Unsent;
                        note.updated_at = now;
                    }
                    if let Some(note) = self.notes.iter().find(|note| note.id == id) {
                        self.persist_note(note, cx);
                    }
                }
                None => {
                    let note = ReviewNote {
                        id: new_note_id(now),
                        target: NoteTarget::DiffLines(draft.target),
                        body,
                        state: ReviewNoteState::Unsent,
                        sent_at: None,
                        created_at: now,
                        updated_at: now,
                    };
                    self.persist_note(&note, cx);
                    self.notes.push(note);
                }
            }
        }
        self.selection = None;
        self.rebuild_rows();
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// 書きかけを捨てる（Esc）。
    pub fn cancel_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft.take().is_some() {
            self.rebuild_rows();
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    pub(crate) fn edit_note(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.notes.get(index) else {
            return;
        };
        let NoteTarget::DiffLines(target) = note.target.clone();
        let (id, body) = (note.id.clone(), note.body.clone());
        self.start_draft(target, Some(id), body, window, cx);
    }

    /// 解決 ⇄ 未解決（戻す先は、一度でも送っていれば送信済み）。
    pub(crate) fn toggle_resolved(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(note) = self.notes.get_mut(index) else {
            return;
        };
        note.state = if note.state == ReviewNoteState::Resolved {
            note.reopened_state()
        } else {
            ReviewNoteState::Resolved
        };
        note.updated_at = now_ms();
        let note = note.clone();
        self.persist_note(&note, cx);
        self.rebuild_rows();
        cx.notify();
    }

    pub(crate) fn delete_note(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.notes.len() {
            return;
        }
        let note = self.notes.remove(index);
        if let Some(storage) = self
            .context
            .as_ref()
            .and_then(|context| context.storage.clone())
        {
            let id = note.id;
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.delete_review_note(&id) {
                        eprintln!("注記を消せない: {error:#}");
                    }
                })
                .detach();
        }
        self.rebuild_rows();
        cx.notify();
    }

    /// 注記の行へ飛ぶ（トレイから）。畳んだファイルなら開いてから。
    pub(crate) fn jump_to_note(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(note) = self.notes.get(index) else {
            return;
        };
        let NoteTarget::DiffLines(target) = &note.target;
        if self.collapsed_files.remove(&target.path) {
            self.rebuild_rows();
        }
        if let Some(row) = self
            .rows
            .iter()
            .position(|row| matches!(row, Row::Note { note, .. } if *note == index))
        {
            self.list.scroll_to(ListOffset {
                item_ix: row.saturating_sub(2),
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }

    pub(crate) fn persist_note(&self, note: &ReviewNote, cx: &mut Context<Self>) {
        let Some(context) = self.context.as_ref() else {
            return;
        };
        let Some(storage) = context.storage.clone() else {
            return;
        };
        let record = match note.to_record(&context.scope) {
            Ok(record) => record,
            Err(error) => {
                eprintln!("注記を保存できない: {error}");
                return;
            }
        };
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = storage.upsert_review_note(&record) {
                    eprintln!("注記を保存できない: {error:#}");
                }
            })
            .detach();
    }

    /// トレイの一覧を開く / 閉じる。
    pub fn toggle_tray(&mut self, cx: &mut Context<Self>) {
        self.tray_open = !self.tray_open;
        cx.notify();
    }

    /// 送る注記（`resend` = 未解決を全部、そうでなければ未送信だけ）。
    pub(crate) fn notes_to_send(&self, resend: bool) -> Vec<&ReviewNote> {
        self.notes
            .iter()
            .filter(|note| {
                if resend {
                    note.is_unresolved()
                } else {
                    note.state == ReviewNoteState::Unsent
                }
            })
            .collect()
    }

    /// 送るボタン: 宛先の一覧を Workspace に頼む。
    pub fn request_send(&mut self, resend: bool, cx: &mut Context<Self>) {
        if self.notes_to_send(resend).is_empty() {
            return;
        }
        self.base_menu = None;
        cx.emit(ReviewEvent::SendMenuRequested { resend });
    }

    /// 宛先のメニューを開く（Workspace が一覧を渡す）。
    pub fn show_send_menu(
        &mut self,
        targets: Vec<SendTarget>,
        resend: bool,
        cx: &mut Context<Self>,
    ) {
        self.send_menu = Some(SendMenu { targets, resend });
        cx.notify();
    }

    /// 宛先を選んだ: 注記を 1 通のプロンプトにして Workspace へ渡す。
    pub(crate) fn send_to(&mut self, target: SendTarget, cx: &mut Context<Self>) {
        let resend = self.send_menu.take().is_some_and(|menu| menu.resend);
        let notes = self.notes_to_send(resend);
        if notes.is_empty() {
            cx.notify();
            return;
        }
        let prompt = format_prompt(&notes);
        let note_ids = notes.iter().map(|note| note.id.clone()).collect();
        cx.emit(ReviewEvent::SendNotes {
            target,
            prompt,
            note_ids,
        });
        cx.notify();
    }

    /// 届いた注記を「送信済み」にする（Workspace が送った後に呼ぶ）。
    pub fn mark_notes_sent(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let now = now_ms();
        let mut changed = Vec::new();
        for note in self.notes.iter_mut().filter(|note| ids.contains(&note.id)) {
            note.state = ReviewNoteState::Sent;
            note.sent_at = Some(now);
            note.updated_at = now;
            changed.push(note.clone());
        }
        for note in &changed {
            self.persist_note(note, cx);
        }
        self.rebuild_rows();
        cx.notify();
    }

    /// 注記の件数（全部, 未送信）。
    pub fn note_counts(&self) -> (usize, usize) {
        let unsent = self
            .notes
            .iter()
            .filter(|note| note.state == ReviewNoteState::Unsent)
            .count();
        (self.notes.len(), unsent)
    }

    /// 指定の行を選ぶ（プローブ・撮影用）。`side` の行番号で探し、`extend` なら範囲を伸ばす。
    pub fn select_line(
        &mut self,
        path: &str,
        side: NoteSide,
        line: u32,
        extend: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(diff) = self.diff.clone() else {
            return false;
        };
        let found = (0..self.rows.len()).find(|index| match &self.rows[*index] {
            Row::Line {
                file,
                hunk,
                line: row_line,
            } => {
                let entry = &diff.files[*file];
                let Some(diff_line) = entry
                    .hunks
                    .get(*hunk)
                    .and_then(|hunk| hunk.lines.get(*row_line))
                else {
                    return false;
                };
                entry.path == path
                    && match side {
                        NoteSide::New => {
                            diff_line.kind != DiffLineKind::Removed
                                && diff_line.new_line == Some(line)
                        }
                        NoteSide::Old => {
                            diff_line.kind == DiffLineKind::Removed
                                && diff_line.old_line == Some(line)
                        }
                    }
            }
            Row::Context { file, new_line, .. } => {
                side == NoteSide::New && diff.files[*file].path == path && *new_line == line
            }
            _ => false,
        });
        match found {
            Some(index) => {
                self.select_row(index, extend, cx);
                self.list.scroll_to_reveal_item(index);
                true
            }
            None => false,
        }
    }

    pub(crate) fn render_note(
        &self,
        row: usize,
        note_index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let Some(note) = self.notes.get(note_index) else {
            return div().into_any_element();
        };
        let resolved = note.state == ReviewNoteState::Resolved;
        div()
            .id(("review-note", row))
            .pl(px(NUMBER_COLUMN_WIDTH * 2. + MARKER_COLUMN_WIDTH))
            .pr(px(16.))
            .py(px(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(7.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .when(resolved, |card| card.opacity(0.6))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.5))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(note_state_label(note.state))),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(CODE_FONT)
                                    .text_size(px(10.5))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(note.target.location())),
                            )
                            .child(div().flex_1())
                            .children(self.note_actions(note_index, resolved, false, cx)),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.fg0)
                            .child(SharedString::from(note.body.clone())),
                    ),
            )
            .into_any_element()
    }

    /// 注記の操作（編集 / 解決・未解決に戻す / 削除）。行の下のカードとトレイの一覧で共用。
    pub(crate) fn note_actions(
        &self,
        note_index: usize,
        resolved: bool,
        in_tray: bool,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let theme = self.theme.clone();
        let action = |id: &'static str, label: String| {
            div()
                .id((id, note_index))
                .flex_none()
                .px(px(6.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .text_size(px(11.))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(label))
        };
        let (edit_id, resolve_id, delete_id) = if in_tray {
            (
                "review-tray-edit",
                "review-tray-resolve",
                "review-tray-delete",
            )
        } else {
            (
                "review-note-edit",
                "review-note-resolve",
                "review-note-delete",
            )
        };
        let resolve_label = if resolved {
            i18n::t!("review.note_reopen")
        } else {
            i18n::t!("review.note_resolve")
        };
        vec![
            action(edit_id, i18n::t!("review.note_edit"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.edit_note(note_index, window, cx);
                    }),
                )
                .into_any_element(),
            action(resolve_id, resolve_label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_resolved(note_index, cx);
                    }),
                )
                .into_any_element(),
            action(delete_id, i18n::t!("review.note_delete"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_note(note_index, cx);
                    }),
                )
                .into_any_element(),
        ]
    }

    pub(crate) fn render_draft(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let Some(draft) = self.draft.as_ref() else {
            return div().into_any_element();
        };
        let title = if draft.editing.is_some() {
            i18n::t!("review.note_editing")
        } else {
            i18n::t!("review.note_new")
        };
        let location = NoteTarget::DiffLines(draft.target.clone()).location();
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_none()
                .px(px(10.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.5))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(label))
        };
        div()
            .id("review-draft-row")
            .pl(px(NUMBER_COLUMN_WIDTH * 2. + MARKER_COLUMN_WIDTH))
            .pr(px(16.))
            .py(px(4.))
            .child(
                div()
                    .id("review-draft")
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(8.))
                    .rounded(px(7.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    // ⌘⏎ は Editor 文脈で `agent::SubmitPrompt` に割り当てられているが、ここには受け手が
                    // 居ないので打鍵のまま上がってくる。Esc は入力欄が複数選択を畳まない時だけ上がる。
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        let keystroke = &event.keystroke;
                        if keystroke.key == "enter" && keystroke.modifiers.secondary() {
                            cx.stop_propagation();
                            this.save_draft(window, cx);
                        } else if keystroke.key == "escape" {
                            cx.stop_propagation();
                            this.cancel_draft(window, cx);
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(title))
                            .child(
                                div()
                                    .font_family(CODE_FONT)
                                    .child(SharedString::from(location)),
                            ),
                    )
                    .child(
                        div()
                            .h(px(72.))
                            .px(px(6.))
                            .py(px(4.))
                            .rounded(px(5.))
                            .bg(theme.bg1)
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(12.5))
                            .child(draft.editor.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.5))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(i18n::t!(
                                        "review.draft_hint",
                                        "save" => keymap_core::keystroke_label("cmd-enter")
                                    ))),
                            )
                            .child(
                                button("review-draft-cancel", i18n::t!("review.draft_cancel"))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.cancel_draft(window, cx)
                                        }),
                                    ),
                            )
                            .child(
                                button("review-draft-save", i18n::t!("review.draft_save"))
                                    .text_color(theme.fg0)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.save_draft(window, cx)
                                        }),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 下端のトレイ（注記の件数・一覧・送る）。注記が 1 件も無ければ出さない。
    pub(crate) fn render_tray(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.notes.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        let (total, unsent) = self.note_counts();
        let resendable = self
            .notes
            .iter()
            .any(|note| note.state == ReviewNoteState::Sent);
        let button = |id: &'static str, label: String, enabled: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(10.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.5))
                .text_color(if enabled { theme.fg0 } else { theme.fg2 })
                .when(enabled, |button| {
                    button.cursor_pointer().hover(|style| style.bg(theme.bg3))
                })
                .child(SharedString::from(label))
        };
        let mut list = div()
            .id("review-tray-list")
            .max_h(px(200.))
            .overflow_y_scroll()
            .py(px(4.))
            .border_b_1()
            .border_color(theme.border);
        for (index, note) in self.notes.iter().enumerate() {
            let resolved = note.state == ReviewNoteState::Resolved;
            let first_line = note.body.lines().next().unwrap_or_default().to_string();
            list = list.child(
                div()
                    .id(("review-tray-note", index))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(26.))
                    .px(px(12.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    .when(resolved, |row| row.opacity(0.6))
                    .child(
                        div()
                            .flex_none()
                            .w(px(64.))
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(note_state_label(note.state))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(CODE_FONT)
                            .text_size(px(11.))
                            .text_color(theme.fg1)
                            .child(SharedString::from(note.target.location())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .text_color(theme.fg0)
                            .child(SharedString::from(first_line)),
                    )
                    .children(self.note_actions(index, resolved, true, cx))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.jump_to_note(index, cx)),
                    ),
            );
        }
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .bg(theme.bg0)
                .border_t_1()
                .border_color(theme.border)
                .when(self.tray_open, |tray| tray.child(list))
                .child(
                    div()
                        .h(px(TRAY_HEIGHT))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .child(
                            div()
                                .id("review-tray-toggle")
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .text_color(theme.fg1)
                                .cursor_pointer()
                                .hover(|style| style.text_color(theme.fg0))
                                .child(if self.tray_open { "▾" } else { "▴" })
                                .child(SharedString::from(i18n::t!(
                                    "review.tray_summary",
                                    "count" => total,
                                    "unsent" => unsent
                                )))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.toggle_tray(cx)),
                                ),
                        )
                        .child(div().flex_1())
                        .when(resendable, |bar| {
                            bar.child(
                                button("review-resend", i18n::t!("review.resend"), true)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.request_send(true, cx)
                                        }),
                                    ),
                            )
                        })
                        .child(
                            button(
                                "review-send",
                                format!("{} ▾", i18n::t!("review.send")),
                                unsent > 0,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.request_send(false, cx)
                                }),
                            ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// 宛先のメニュー（トレイの右上に開く）。
    pub(crate) fn render_send_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.send_menu.as_ref()?;
        let theme = self.theme.clone();
        let count = self.notes_to_send(menu.resend).len();
        let mut body = div().flex().flex_col().p(px(4.)).child(
            div()
                .px(px(10.))
                .pt(px(6.))
                .pb(px(4.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg2)
                .child(SharedString::from(
                    i18n::t!("review.send_to", "count" => count),
                )),
        );
        if menu.targets.is_empty() {
            body = body.child(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("review.send_empty"))),
            );
        }
        for (index, target) in menu.targets.iter().enumerate() {
            let chosen = target.clone();
            let detail = if target.is_default {
                SharedString::from(format!(
                    "{} · {}",
                    target.detail,
                    i18n::t!("review.send_default")
                ))
            } else {
                target.detail.clone()
            };
            body = body.child(
                div()
                    .id(("review-send-target", index))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(theme.fg0)
                    .cursor_pointer()
                    .when(target.is_default, |item| item.bg(self.accent.alpha(0.16)))
                    .hover(|style| style.bg(theme.bg3))
                    .child(div().flex_none().size(px(7.)).rounded_full().bg(
                        if target.thread.is_some() {
                            target.color
                        } else {
                            theme.fg2
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(target.label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(theme.fg2)
                            .child(detail),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.send_to(chosen.clone(), cx);
                        }),
                    ),
            );
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.send_menu = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("review-send-menu")
                        .absolute()
                        .right(px(10.))
                        .bottom(px(TRAY_HEIGHT + 4.))
                        .w(px(340.))
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(6.),
                            gpui::hsla(0., 0., 0., 0.35),
                        )
                        .blur_radius(px(16.))])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(body),
                )
                .into_any_element(),
        )
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 注記の id（時刻 + プロセス + 通し番号。同じミリ秒に 2 件作っても重ならない）。
pub(crate) fn new_note_id(now: i64) -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("note-{now}-{}-{sequence}", std::process::id())
}

pub(crate) fn note_state_label(state: ReviewNoteState) -> String {
    match state {
        ReviewNoteState::Unsent => i18n::t!("review.note_unsent"),
        ReviewNoteState::Sent => i18n::t!("review.note_sent"),
        ReviewNoteState::Resolved => i18n::t!("review.note_resolved"),
    }
}
