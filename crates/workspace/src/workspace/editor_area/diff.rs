use crate::workspace::*;

impl Workspace {
    pub(crate) fn open_diff_tab(
        &mut self,
        _: &OpenDiff,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let (path, current) = {
            let view = editor.read(cx);
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            (path, view.buffer().text())
        };
        self.open_diff_tab_for(path, Some(current), window, cx);
    }

    /// 指定ファイルの diff タブを開く（git パネル行から）。`current` が None ならディスクの内容を使う。
    pub(crate) fn open_diff_tab_for(
        &mut self,
        path: PathBuf,
        current: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            let (diff_text, path) = cx
                .background_executor()
                .spawn(async move {
                    let current = current.or_else(|| {
                        host.read_file(&path)
                            .ok()
                            .and_then(|content| String::from_utf8(content.bytes).ok())
                    });
                    let diff = current.as_deref().and_then(|current| {
                        project::unified_diff_on(host.as_ref(), &path, current)
                    });
                    (diff, path)
                })
                .await;
            let Some(diff_text) = diff_text else {
                eprintln!("diff: 差分なし（HEAD と同一） {}", path.display());
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                let title = path.with_file_name(format!("{name} ⇄ HEAD"));
                let mut buffer = Buffer::from_str(&diff_text);
                buffer.set_read_only(true);
                workspace.open_transient_tab(title, buffer, window, cx);
            });
        })
        .detach();
    }

    /// diff タブ内で次/前の hunk ヘッダ（@@ 行）へ移動する（F7/⇧F7）。
    pub(crate) fn step_hunk_header(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |view, cx| {
            let snapshot = view.buffer().snapshot();
            let current_row = snapshot
                .byte_to_point(
                    view.buffer()
                        .selections()
                        .first()
                        .map(|s| s.head)
                        .unwrap_or(0),
                )
                .row;
            let rows: Vec<usize> = (0..snapshot.line_count())
                .filter(|row| snapshot.line_text(*row).starts_with("@@"))
                .collect();
            if rows.is_empty() {
                return;
            }
            let target = if delta > 0 {
                rows.iter()
                    .copied()
                    .find(|row| *row > current_row)
                    .unwrap_or(rows[0])
            } else {
                rows.iter()
                    .rev()
                    .copied()
                    .find(|row| *row < current_row)
                    .unwrap_or(*rows.last().unwrap())
            };
            view.reveal_position(target, 0, cx);
        });
    }

    /// gutter の diff バークリック（hunk ポップオーバー）。
    pub(crate) fn on_hunk_clicked(
        &mut self,
        hunk: project::DiffHunk,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.hunk_menu = Some((hunk, position));
        cx.notify();
    }

    /// hunk を stage する（この hunk だけ index へ）。
    pub(crate) fn stage_hunk(&mut self, hunk: project::DiffHunk, cx: &mut Context<Self>) {
        self.hunk_menu = None;
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let (path, current) = {
            let view = editor.read(cx);
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            (path, view.buffer().text())
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let head = project::head_text_on(host.as_ref(), &path).unwrap_or_default();
                    let repo = path
                        .ancestors()
                        .find(|dir| dir.join(".git").exists())
                        .map(Path::to_path_buf)
                        .unwrap_or(root);
                    let relative = path
                        .strip_prefix(&repo)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    let head_lines: Vec<&str> = head.lines().collect();
                    let current_lines: Vec<&str> = current.lines().collect();
                    let patch =
                        project::hunk_patch_text(&relative, &head_lines, &current_lines, &hunk);
                    project::apply_patch_to_index_on(host.as_ref(), &repo, &patch)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    eprintln!("hunk stage に失敗: {error:#}");
                }
                workspace.refresh_git_status(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// hunk を巻き戻す（バッファ上で HEAD の内容に戻す・undo 可）。
    pub(crate) fn revert_hunk(&mut self, hunk: project::DiffHunk, cx: &mut Context<Self>) {
        self.hunk_menu = None;
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        editor.update(cx, |view, cx| {
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            let head = project::head_text_on(host.as_ref(), &path).unwrap_or_default();
            let head_lines: Vec<&str> = head.lines().collect();
            let replacement: String = head_lines
                .iter()
                .skip(hunk.old_range.start as usize)
                .take(hunk.old_range.len())
                .map(|line| format!("{line}\n"))
                .collect();
            let snapshot = view.buffer().snapshot();
            let start_row = hunk.new_range.start as usize;
            let end_row = hunk.new_range.end as usize;
            let start = snapshot.point_to_byte(editor_core::Point::new(
                start_row.min(snapshot.line_count().saturating_sub(1)),
                0,
            ));
            let end = if hunk.new_range.is_empty() {
                start // 削除 hunk: その位置に HEAD の行を挿入
            } else if end_row < snapshot.line_count() {
                snapshot.point_to_byte(editor_core::Point::new(end_row, 0))
            } else {
                view.buffer().len_bytes()
            };
            view.replace_ranges(&[start..end], &replacement, cx);
        });
    }

    /// hunk のバッファ側テキストをコピーする。
    pub(crate) fn copy_hunk(&mut self, hunk: project::DiffHunk, cx: &mut Context<Self>) {
        self.hunk_menu = None;
        let Some(editor) = self.active_editor() else {
            return;
        };
        let text = {
            let view = editor.read(cx);
            let snapshot = view.buffer().snapshot();
            (hunk.new_range.start as usize..hunk.new_range.end as usize)
                .filter(|row| *row < snapshot.line_count())
                .map(|row| format!("{}\n", snapshot.line_text(row)))
                .collect::<String>()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.notify();
    }

    /// hunk 操作ポップオーバー（gutter クリックで開く・M11-10）。
    pub(crate) fn render_hunk_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (hunk, position) = self.hunk_menu.clone()?;
        let theme = self.theme.clone();
        let item = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .px(px(9.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(label))
        };
        let stage_hunk = hunk.clone();
        let revert_hunk_data = hunk.clone();
        let copy_hunk_data = hunk;
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.hunk_menu = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(position.x + px(8.))
                        .top(position.y)
                        .w(px(220.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .p(px(4.))
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(6.),
                            gpui::hsla(0., 0., 0., 0.4),
                        )
                        .blur_radius(px(16.))])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(item("hunk-stage", i18n::t!("hunk.stage")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.stage_hunk(stage_hunk.clone(), cx)
                            }),
                        ))
                        .child(item("hunk-revert", i18n::t!("hunk.revert")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.revert_hunk(revert_hunk_data.clone(), cx)
                            }),
                        ))
                        .child(item("hunk-copy", i18n::t!("hunk.copy")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.copy_hunk(copy_hunk_data.clone(), cx)
                            }),
                        ))
                        .child(item("hunk-diff", i18n::t!("hunk.open_diff")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.hunk_menu = None;
                                this.open_diff_tab(&OpenDiff, window, cx)
                            }),
                        )),
                )
                .into_any_element(),
        )
    }

    // ── blame（キャレット行の行末に 由来 を dim 表示・M11-11） ──

    /// キャレット行が変わっていたら 400ms デバウンスで `git blame -L` を背景実行し、
    /// 行末注釈へ反映する。dirty バッファでは HEAD 基準の近似（行ずれ許容）。
    pub(crate) fn schedule_blame(&mut self, editor: &Entity<EditorView>, cx: &mut Context<Self>) {
        if self.active_editor().as_ref() != Some(editor) {
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let target = {
            let view = editor.read(cx);
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                // 無題/一時タブは注釈なし。
                return;
            };
            let head = view
                .buffer()
                .selections()
                .first()
                .map(|s| s.head)
                .unwrap_or(0);
            let row = view.buffer().snapshot().byte_to_point(head).row;
            (path, row)
        };
        if self.last_blame_target.as_ref() == Some(&target) {
            return; // 同じ行（blink や横移動）では再計算しない
        }
        self.last_blame_target = Some(target.clone());
        self.blame_gen = self.blame_gen.wrapping_add(1);
        let generation = self.blame_gen;
        let host = worktree.host().clone();
        let editor = editor.clone();
        let (path, row) = target;
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            let latest = workspace
                .update(cx, |workspace, _| workspace.blame_gen == generation)
                .unwrap_or(false);
            if !latest {
                return;
            }
            let annotation = cx
                .background_executor()
                .spawn(async move { project::blame_line_on(host.as_ref(), &path, row as u32 + 1) })
                .await;
            let still_latest = workspace
                .update(cx, |workspace, _| workspace.blame_gen == generation)
                .unwrap_or(false);
            if !still_latest {
                return;
            }
            let _ = editor.update(cx, |view, cx| {
                let text = annotation.map(|line| match line {
                    project::BlameLine::Uncommitted => i18n::t!("editor.blame_uncommitted"),
                    project::BlameLine::Commit(text) => text,
                });
                view.set_line_annotation(text.map(|text| (row, SharedString::from(text))), cx);
            });
        })
        .detach();
    }

    // ── シンボル（⌘⇧O アウトライン / ⌘T ワークスペース・M11） ──

    // ⌘⇧O: tree-sitter アウトラインを Picker で（LSP 不要・対応言語のみ）。
}
