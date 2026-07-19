impl Workspace {
    fn open_outline(&mut self, _: &OutlineSymbols, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let items = {
            let view = editor.read(cx);
            let Some(extension) = view
                .buffer()
                .path()
                .and_then(|path| path.extension())
                .and_then(|extension| extension.to_str())
            else {
                return;
            };
            lang::outline(&view.buffer().text(), extension)
        };
        if items.is_empty() {
            return;
        }
        self.picker_symbol_rows = items.iter().map(|item| item.row).collect();
        let picker_items = items
            .iter()
            .enumerate()
            .map(|(id, item)| {
                PickerItem::new(id, format!("{} {}", item.kind, item.name))
                    .with_detail(format!("{}", item.row + 1))
            })
            .collect();
        self.open_picker(PickerMode::Symbols, i18n::t!("symbols.outline"), picker_items, window, cx);
    }

    /// ⌘T: LSP workspace/symbol を Picker で（空クエリの初期一覧 → Picker の fuzzy で絞る）。
    fn open_workspace_symbols(&mut self, _: &WorkspaceSymbols, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        if !self.lsp_initialized {
            return;
        }
        let Some(lsp) = &self.lsp else {
            return;
        };
        let receiver = lsp.workspace_symbols("");
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                let Some(array) = value.as_array() else {
                    return;
                };
                let mut locations = Vec::new();
                let mut items = Vec::new();
                for (id, symbol) in array.iter().take(200).enumerate() {
                    let Some(name) = symbol.get("name").and_then(|name| name.as_str()) else {
                        continue;
                    };
                    let Some(uri) = symbol.pointer("/location/uri").and_then(|uri| uri.as_str())
                    else {
                        continue;
                    };
                    let Some(path) = lang::lsp::uri_to_path(uri) else {
                        continue;
                    };
                    let line = symbol
                        .pointer("/location/range/start/line")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as u32;
                    let character = symbol
                        .pointer("/location/range/start/character")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as u32;
                    let container = symbol
                        .get("containerName")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    locations.push((path.clone(), line, character));
                    let file = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default();
                    items.push(
                        PickerItem::new(id, name.to_string())
                            .with_detail(if container.is_empty() { file } else { format!("{container} — {file}") }),
                    );
                }
                if items.is_empty() {
                    return;
                }
                workspace.picker_workspace_symbols = locations;
                workspace.open_picker(
                    PickerMode::WorkspaceSymbols,
                    i18n::t!("symbols.workspace"),
                    items,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    // ── 診断一覧 + F8（M11） ──

    /// F8/⇧F8: アクティブファイルの次/前の診断行へ。
    fn step_diagnostic(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(path) = self.active_tab_path() else {
            return;
        };
        let Some(entries) = self.diagnostics.get(&path) else {
            return;
        };
        let mut rows: Vec<u32> = entries.iter().map(|(line, _)| *line).collect();
        rows.sort_unstable();
        rows.dedup();
        if rows.is_empty() {
            return;
        }
        let current_row = {
            let view = editor.read(cx);
            let head = view.buffer().selections().first().map(|s| s.head).unwrap_or(0);
            view.buffer().snapshot().byte_to_point(head).row as u32
        };
        let target = if delta > 0 {
            rows.iter().copied().find(|row| *row > current_row).unwrap_or(rows[0])
        } else {
            rows.iter().rev().copied().find(|row| *row < current_row).unwrap_or(*rows.last().unwrap())
        };
        editor.update(cx, |view, cx| view.reveal_position(target as usize, 0, cx));
    }

    fn next_diagnostic(&mut self, _: &NextDiagnostic, _: &mut Window, cx: &mut Context<Self>) {
        self.step_diagnostic(1, cx);
    }

    fn prev_diagnostic(&mut self, _: &PrevDiagnostic, _: &mut Window, cx: &mut Context<Self>) {
        self.step_diagnostic(-1, cx);
    }

    /// statusbar の ✗▲ クリック: 全ファイルの診断を検索パネル UI で一覧（メッセージがプレビュー行）。
    fn open_diagnostics_panel(&mut self, _: &DiagnosticsPanel, window: &mut Window, cx: &mut Context<Self>) {
        let mut results: Vec<search::FileMatch> = Vec::new();
        let mut files: Vec<&PathBuf> = self.raw_diagnostics.keys().collect();
        files.sort();
        for path in files {
            let Some(array) = self.raw_diagnostics.get(path).and_then(|value| value.as_array())
            else {
                continue;
            };
            let mut matches: Vec<search::Match> = array
                .iter()
                .filter_map(|diagnostic| {
                    let line = diagnostic.pointer("/range/start/line")?.as_u64()? as usize;
                    let message = diagnostic.get("message")?.as_str()?;
                    let severity = diagnostic
                        .get("severity")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(1);
                    let icon = if severity == 1 { "✗" } else { "▲" };
                    Some(search::Match {
                        line,
                        column: 0,
                        byte_range: 0..0,
                        line_text: format!("{icon} {}", message.lines().next().unwrap_or("")),
                    })
                })
                .collect();
            if matches.is_empty() {
                continue;
            }
            matches.sort_by_key(|found| found.line);
            results.push(search::FileMatch { path: path.clone(), matches });
        }
        if results.is_empty() {
            return;
        }
        let query = i18n::t!("diagnostics.title");
        self.show_search_results(query, results, window, cx);
    }

    // ── code actions（⌘.・M11） ──

    fn open_code_actions(&mut self, _: &CodeActions, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
            return;
        };
        let info = {
            let view = editor.read(cx);
            let position = view.caret_window_position();
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(|path| {
                    let (line, character) = view.cursor_lsp_position();
                    (path.to_path_buf(), line, character, position)
                })
        };
        let debug = std::env::var_os("SHIRUSHI_LSP_DEBUG").is_some();
        let (Some((path, line, character, position)), true) = (info, self.lsp_initialized) else {
            if debug {
                eprintln!("code_actions: LSP 未初期化 or 対応外ファイル");
            }
            return;
        };
        let Some(lsp) = &self.lsp else {
            if debug {
                eprintln!("code_actions: LSP クライアント無し");
            }
            return;
        };
        // 同じ行の診断だけを context に渡す（quickfix の対象特定）。
        let diagnostics = self
            .raw_diagnostics
            .get(&path)
            .and_then(|value| value.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic
                            .pointer("/range/start/line")
                            .and_then(|v| v.as_u64())
                            .map(|l| l as u32 == line)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let receiver = lsp.code_actions(&path, line, character, serde_json::Value::Array(diagnostics));
        cx.spawn(async move |_workspace, cx| {
            let response = receiver.await;
            if std::env::var_os("SHIRUSHI_LSP_DEBUG").is_some() {
                eprintln!("code_actions 応答: {response:?}");
            }
            let Ok(Ok(value)) = response else {
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                let Some(array) = value.as_array() else {
                    return;
                };
                let items: Vec<(SharedString, serde_json::Value)> = array
                    .iter()
                    .filter_map(|action| {
                        let title = action.get("title")?.as_str()?.to_string();
                        Some((SharedString::from(title), action.clone()))
                    })
                    .take(20)
                    .collect();
                if items.is_empty() {
                    return;
                }
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                let position = workspace
                    .active_editor()
                    .and_then(|editor| editor.read(cx).caret_window_position())
                    .or(position)
                    .unwrap_or_else(|| point(px(220.), px(180.)));
                workspace.code_actions = Some(CodeActionsState { items, selected: 0, position, focus });
                cx.notify();
            });
        })
        .detach();
    }

    fn close_code_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_actions.take().is_none() {
            return;
        }
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// 選択中アクションを適用する。edit が無ければ codeAction/resolve で解決してから。
    fn confirm_code_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let action = self
            .code_actions
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .map(|(_, action)| action.clone());
        self.close_code_actions(window, cx);
        let Some(action) = action else {
            return;
        };
        if let Some(edit) = action.get("edit").filter(|edit| !edit.is_null()) {
            let edit = edit.clone();
            self.apply_workspace_edit(&edit, cx);
            return;
        }
        // edit が遅延解決（ra はこのパターンが多い）→ resolve してから適用。
        let Some(lsp) = &self.lsp else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let receiver = lsp.resolve_code_action(action);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(resolved)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, _window, cx| {
                if let Some(edit) = resolved.get("edit").filter(|edit| !edit.is_null()) {
                    let edit = edit.clone();
                    workspace.apply_workspace_edit(&edit, cx);
                } else {
                    eprintln!("code action: 適用できる edit が無い（command のみは v1 非対応）");
                }
            });
        })
        .detach();
    }

    fn on_code_actions_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_code_actions(window, cx),
            "up" => {
                if let Some(state) = self.code_actions.as_mut() {
                    let len = state.items.len();
                    state.selected = (state.selected + len - 1) % len.max(1);
                    cx.notify();
                }
            }
            "down" => {
                if let Some(state) = self.code_actions.as_mut() {
                    let len = state.items.len().max(1);
                    state.selected = (state.selected + 1) % len;
                    cx.notify();
                }
            }
            "enter" => self.confirm_code_action(window, cx),
            _ => self.close_code_actions(window, cx),
        }
    }

    /// ⌘. のポップアップ（補完と同じ見た目・キャレット直下）。
    fn render_code_actions(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.code_actions.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let focus = state.focus.clone();
        let selected = state.selected;
        let list = div().flex().flex_col().max_h(px(280.)).overflow_hidden().children(
            state.items.iter().enumerate().map(|(row, (title, _))| {
                let is_selected = row == selected;
                div()
                    .id(("code-action", row))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(is_selected, |element| element.bg(accent.alpha(0.16)))
                    .hover(|style| style.bg(theme.bg3))
                    .child(div().flex_none().text_size(px(10.)).text_color(accent).child("✦"))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .text_color(if is_selected { theme.fg0 } else { theme.fg1 })
                            .child(title.clone()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if let Some(state) = this.code_actions.as_mut() {
                                state.selected = row;
                            }
                            this.confirm_code_action(window, cx)
                        }),
                    )
            }),
        );
        Some(
            div()
                .absolute()
                .inset_0()
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_code_actions_key_down))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_code_actions(window, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .left(state.position.x)
                        .top(state.position.y + px(2.))
                        .w(px(380.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .p(px(4.))
                        .shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
                        ])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(list),
                )
                .into_any_element(),
        )
    }

    // ── 参照検索（⇧F12・M11） ──

    /// 参照を検索して ⌘⇧F の結果パネル（ファイル別グルーピング UI）で見せる。
    fn find_references(&mut self, _: &FindReferences, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
            return;
        };
        let info = {
            let view = editor.read(cx);
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(|path| {
                    let (line, character) = view.cursor_lsp_position();
                    let snapshot = view.buffer().snapshot();
                    let head = view.buffer().selections().first().map(|s| s.head).unwrap_or(0);
                    let word = snapshot
                        .word_range_at(head)
                        .map(|range| view.buffer().text_range(range))
                        .unwrap_or_default();
                    (path.to_path_buf(), line, character, word)
                })
        };
        let (Some((path, line, character, word)), true) = (info, self.lsp_initialized) else {
            return;
        };
        let Some(lsp) = &self.lsp else {
            return;
        };
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let receiver = lsp.references(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            // Location[] → ファイル別に集約し、プレビュー行のために背景でファイルを読む。
            let results = cx
                .background_executor()
                .spawn(async move { locations_to_file_matches(host.as_ref(), &value) })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                let query = i18n::t!("searchpanel.reference_query", "word" => word);
                workspace.show_search_results(query, results, window, cx);
            });
        })
        .detach();
    }

    /// hover をキーで出す（⌘K ⌘I）。キャレット位置で要求し、キャレット直上に出す。
    fn show_hover_at_caret(&mut self, _: &ShowHover, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let (line, character, anchor) = {
            let view = editor.read(cx);
            let (line, character) = view.cursor_lsp_position();
            (line, character, view.caret_window_position())
        };
        let Some(anchor) = anchor else {
            return;
        };
        self.request_hover(line, character, anchor, window, cx);
    }

    /// エディタの hover dwell（[`EditorHoverEvent::Dwell`]）→ LSP hover 要求（M10）。
    fn on_editor_hover(
        &mut self,
        editor: &Entity<EditorView>,
        event: &EditorHoverEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (line, character, anchor) = match *event {
            EditorHoverEvent::Dwell { line, character, anchor } => (line, character, anchor),
            EditorHoverEvent::Cancel => {
                self.close_hover(cx);
                return;
            }
        };
        if self.active_editor().as_ref() != Some(editor) {
            return;
        }
        // 補完ポップアップが開いている間は hover を出さない（重なると鬱陶しい）。
        if self.completion.is_some() {
            return;
        }
        self.request_hover(line, character, anchor, window, cx);
    }

    /// LSP へ hover を要求し、応答があればポップアップを出す（dwell / ⌘K ⌘I 共通）。
    fn request_hover(
        &mut self,
        line: u32,
        character: u32,
        anchor: Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
            return;
        };
        let path = {
            let view = editor.read(cx);
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(Path::to_path_buf)
        };
        let Some(path) = path else {
            return;
        };
        if !self.lsp_initialized {
            return;
        }
        self.hover_generation = self.hover_generation.wrapping_add(1);
        let generation = self.hover_generation;
        let Some(lsp) = &self.lsp else {
            return;
        };
        let receiver = lsp.hover(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, _window, cx| {
                if workspace.hover_generation != generation {
                    return;
                }
                let lines = parse_hover_lines(&value);
                if lines.is_empty() {
                    return;
                }
                const MAX_HOVER_LINES: usize = 24;
                let mut lines: Vec<SharedString> = lines
                    .into_iter()
                    .take(MAX_HOVER_LINES + 1)
                    .map(SharedString::from)
                    .collect();
                if lines.len() > MAX_HOVER_LINES {
                    lines.truncate(MAX_HOVER_LINES);
                    lines.push(SharedString::from("…"));
                }
                workspace.hover = Some(HoverState { lines, position: anchor });
                cx.notify();
            });
        })
        .detach();
    }

    // ── ファイル監視（watch 基盤・M10） ──

    // アクティブプロジェクトの worktree 監視を開始する（既存の監視は破棄して張り替え）。
    // local のみ（remote の watch subscription は M13）。イベントは 200ms 合流してから反映する。
}
