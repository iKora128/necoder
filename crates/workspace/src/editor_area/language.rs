impl Workspace {
    fn ensure_lsp(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let root = worktree.root().to_path_buf();
        let Some(path) = self
            .active_editor()
            .and_then(|editor| editor.read(cx).buffer().path().map(Path::to_path_buf))
        else {
            return;
        };
        let Some(server) = language_server_for(&path, worktree.host().is_remote()) else {
            return; // この拡張子に LSP は無い（＝静かに素通り）
        };
        // 同じ root × 同じ言語なら張り替え不要。
        if self.lsp.is_some()
            && self.lsp_root.as_deref() == Some(root.as_path())
            && self.lsp_language == Some(server.language_id)
        {
            return;
        }
        // 旧接続を畳む（Drop で kill）。
        self.lsp = None;
        self._lsp_pump = None;
        self.lsp_initialized = false;
        self.lsp_sent_versions.clear();
        self.diagnostics.clear();
        self.search_panel = None;
        let (client, notifications) = match lang::lsp::LspClient::new_on(
            worktree.host().clone(),
            &server.command,
            &server.args,
            &root,
        ) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("LSP の起動に失敗（{}）: {error:#}", server.language_id);
                return;
            }
        };
        let init_rx = client.initialize_request(&root);
        self.lsp = Some(client);
        self.lsp_root = Some(root);
        self.lsp_language = Some(server.language_id);
        // pump: initialize 応答 → initialized + 現在ファイル didOpen → 以後 publishDiagnostics を処理。
        self._lsp_pump = Some(cx.spawn(async move |workspace, cx| {
            // capabilities から didChange の sync 種別を読む（2 = Incremental・M11-8）。
            let incremental = match init_rx.await {
                Ok(Ok(value)) => {
                    let sync = value.pointer("/capabilities/textDocumentSync");
                    let kind = sync
                        .and_then(|sync| sync.as_u64())
                        .or_else(|| sync.and_then(|sync| sync.pointer("/change")).and_then(|c| c.as_u64()));
                    kind == Some(2)
                }
                _ => false,
            };
            if workspace
                .update(cx, |ws, cx| {
                    ws.lsp_incremental_sync = incremental;
                    ws.on_lsp_initialized(cx)
                })
                .is_err()
            {
                return;
            }
            let mut notifications = notifications;
            while let Some((method, params)) = notifications.next().await {
                if method == "textDocument/publishDiagnostics"
                    && workspace.update(cx, |ws, cx| ws.on_diagnostics(params, cx)).is_err()
                {
                    break;
                }
            }
        }));
    }

    /// initialize 応答後: initialized 通知 + 現在の rust ファイルを didOpen。
    fn on_lsp_initialized(&mut self, cx: &mut Context<Self>) {
        self.lsp_initialized = true;
        if let Some(lsp) = &self.lsp {
            lsp.initialized();
        }
        self.lsp_did_open_active(cx);
    }

    /// 現在開いているファイルを didOpen する（稼働中サーバが担当する言語に限る）。
    fn lsp_did_open_active(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let info = {
            let view = editor.read(cx);
            match view.buffer().path() {
                // 稼働中サーバが担当する言語のときだけ開く。
                Some(path) => language_server_for(path, view.buffer().host().is_remote())
                    .filter(|server| self.lsp_language == Some(server.language_id))
                    .map(|server| {
                        (path.to_path_buf(), server.language_id, view.buffer().version(), view.buffer().text())
                    }),
                None => None,
            }
        };
        if let Some((path, language_id, version, text)) = info {
            self.lsp_sent_versions.insert(path.clone(), version);
            if let Some(lsp) = &self.lsp {
                lsp.did_open(&path, language_id, version as i32, &text);
            }
        }
    }

    /// タブを閉じたら didClose を送り、送信済み version 記録も外す（稼働中サーバ担当言語のみ）。
    fn lsp_did_close(&mut self, path: &Path) {
        self.lsp_sent_versions.remove(path);
        if !self.lsp_initialized {
            return;
        }
        let closes = language_server_for(path, self.active_worktree().map(|w| w.host().is_remote()).unwrap_or(false))
            .map(|server| self.lsp_language == Some(server.language_id))
            .unwrap_or(false);
        if closes {
            if let Some(lsp) = &self.lsp {
                lsp.did_close(path);
            }
        }
    }

    /// publishDiagnostics を受けてファイル別に格納し、アクティブファイル分をエディタへ push。
    fn on_diagnostics(&mut self, params: serde_json::Value, cx: &mut Context<Self>) {
        // 生 JSON（Diagnostic[]）も保持する（⌘. の codeAction context 用・M11）。
        let raw = params.get("diagnostics").cloned().unwrap_or(serde_json::Value::Array(Vec::new()));
        let raw_uri = params.get("uri").and_then(|uri| uri.as_str()).map(str::to_string);
        let Ok(parsed) = serde_json::from_value::<lang::lsp::PublishDiagnosticsParams>(params) else {
            return;
        };
        let Some(path) = lang::lsp::uri_to_path(&parsed.uri) else {
            return;
        };
        if raw_uri.as_deref() == Some(parsed.uri.as_str()) {
            if raw.as_array().map(|array| array.is_empty()).unwrap_or(true) {
                self.raw_diagnostics.remove(&path);
            } else {
                self.raw_diagnostics.insert(path.clone(), raw);
            }
        }
        let entries: Vec<(u32, lang::lsp::Severity)> = parsed
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (diagnostic.range.start.line, lang::lsp::Severity::from_lsp(diagnostic.severity))
            })
            .collect();
        // 空 vec = そのファイルの診断を全消し（置換セマンティクス）。
        if entries.is_empty() {
            self.diagnostics.remove(&path);
        } else {
            self.diagnostics.insert(path, entries);
        }
        self.push_active_diagnostics(cx);
        cx.notify();
    }

    /// アクティブファイルの診断をエディタへ渡す（gutter 下線用）。
    fn push_active_diagnostics(&self, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let path = editor.read(cx).buffer().path().map(Path::to_path_buf);
        let diagnostics = path
            .and_then(|path| self.diagnostics.get(&path).cloned())
            .unwrap_or_default();
        editor.update(cx, |view, cx| view.set_diagnostics(diagnostics, cx));
    }

    /// エディタ変更時（observe）: 再描画 + LSP didChange（rust・version 変化時のみ）。
    fn on_editor_changed(&mut self, editor: Entity<EditorView>, cx: &mut Context<Self>) {
        cx.notify();
        // hot exit: **バッファ version が変わった時だけ**スナップショットを予約する。
        // observe は blink（530ms 毎）でも発火するので、無ガードだと 2s デバウンスが永遠に流れる。
        {
            let (path, version) = {
                let view = editor.read(cx);
                (view.buffer().path().map(Path::to_path_buf), view.buffer().version())
            };
            if let Some(path) = path {
                if self.hot_exit_versions.get(&path) != Some(&version) {
                    self.hot_exit_versions.insert(path, version);
                    self.schedule_hot_exit_snapshot(cx);
                }
            }
        }
        // blame: キャレット行が変わったら（デバウンス付きで）行末注釈を更新（M11-11）。
        self.schedule_blame(&editor, cx);
        // ⌘F が開いていれば、アクティブエディタの編集にマッチを追従させる
        // （version ガードで blink/focus の notify は素通り・再入も止まる）。
        if self.buffer_search.is_some()
            && self.active_editor().is_some_and(|active| active == editor)
        {
            self.refresh_buffer_search(false, cx);
        }
        // 初期化 + didOpen 前は didChange を送らない（さもないと ra が「initialized 前」で落ちる）。
        // observe は focus/blink 等の notify でも発火するので version 変化でのみ送る。
        if !self.lsp_initialized {
            return;
        }
        // 単一編集 + サーバが Incremental 広告 → range 差分で送る（M11-8）。
        // それ以外（複数編集/undo/redo/reload・Full サーバ）は全文。
        enum Change {
            Incremental { start: (u32, u32), end: (u32, u32), text: String },
            Full(String),
        }
        let info = {
            let view = editor.read(cx);
            let version = view.buffer().version();
            view.buffer()
                .path()
                .filter(|path| {
                    language_server_for(path, view.buffer().host().is_remote()).is_some()
                })
                // ファイル別に「送信済み version」と比較（複数タブで version 番号が衝突しても誤スキップしない）。
                .filter(|path| self.lsp_sent_versions.get(*path) != Some(&version))
                .map(|path| {
                    let change = match view.buffer().last_change() {
                        Some(edits) if edits.len() == 1 && self.lsp_incremental_sync => {
                            let (start, old, new) = (&edits[0].0, &edits[0].1, &edits[0].2);
                            // start までは編集前後で同一 → 現バッファから UTF-16 位置が取れる。
                            let (start_line, start_character) = view.lsp_position_for_offset(*start);
                            let newlines = old.matches('\n').count() as u32;
                            let end = if newlines == 0 {
                                (start_line, start_character + old.encode_utf16().count() as u32)
                            } else {
                                let tail = &old[old.rfind('\n').map(|i| i + 1).unwrap_or(0)..];
                                (start_line + newlines, tail.encode_utf16().count() as u32)
                            };
                            Change::Incremental {
                                start: (start_line, start_character),
                                end,
                                text: new.clone(),
                            }
                        }
                        _ => Change::Full(view.buffer().text()),
                    };
                    (path.to_path_buf(), version, change)
                })
        };
        if let Some((path, version, change)) = info {
            self.lsp_sent_versions.insert(path.clone(), version);
            if let Some(lsp) = &self.lsp {
                match change {
                    Change::Incremental { start, end, text } => lsp.did_change_incremental(
                        &path,
                        version as i32,
                        start.0,
                        start.1,
                        end.0,
                        end.1,
                        &text,
                    ),
                    Change::Full(text) => lsp.did_change(&path, version as i32, &text),
                }
            }
        }
    }

    /// アクティブファイルの診断件数（error, warning）。statusbar 用。
    fn active_diagnostic_counts(&self, cx: &App) -> (usize, usize) {
        let Some(editor) = self.active_editor() else {
            return (0, 0);
        };
        let Some(path) = editor.read(cx).buffer().path().map(Path::to_path_buf) else {
            return (0, 0);
        };
        let Some(entries) = self.diagnostics.get(&path) else {
            return (0, 0);
        };
        let errors = entries.iter().filter(|(_, severity)| *severity == lang::lsp::Severity::Error).count();
        let warnings =
            entries.iter().filter(|(_, severity)| *severity == lang::lsp::Severity::Warning).count();
        (errors, warnings)
    }

    /// 定義ジャンプ（F12）。カーソル位置の定義を rust-analyzer に問い合わせて着地する。
    fn go_to_definition(&mut self, _: &GoToDefinition, window: &mut Window, cx: &mut Context<Self>) {
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
                (path.to_path_buf(), line, character)
            })
        };
        let Some((path, line, character)) = info else {
            return;
        };
        let Some(lsp) = &self.lsp else {
            return;
        };
        let receiver = lsp.definition(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            if let Some(location) = parse_definition(&value) {
                let _ = handle.update(cx, |workspace, window, cx| {
                    workspace.record_nav_position(cx); // F12 はナビ履歴へ（⌃- で戻れる）
                    workspace.jump_to_location(
                        location.path,
                        location.position.line,
                        location.position.character,
                        window,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// 定義の着地: 別ファイルなら開き、対象位置を中央へ寄せる。
    fn jump_to_location(
        &mut self,
        path: PathBuf,
        line: u32,
        character: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.active_tab_path();
        if current.as_deref() != Some(path.as_path()) {
            // 別ファイル: 背景読み完了後のエディタへ着地（旧バッファへの誤 reveal 防止）。
            self.open_file_then(path, window, cx, move |view, cx| {
                view.reveal_lsp_position(line, character, cx)
            });
        } else if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.reveal_lsp_position(line, character, cx));
        }
        cx.notify();
    }

    /// 補完（Ctrl-Space）。カーソル位置で候補を取得しポップアップを出す。
    /// Ctrl-Space（手動トリガ）。Esc 抑止を解除して要求する。
    fn trigger_completion(&mut self, _: &TriggerCompletion, window: &mut Window, cx: &mut Context<Self>) {
        self.completion_suppressed_word = None;
        self.request_completion(window, cx);
    }

    /// LSP へ補完を要求し、応答でポップアップを出す（手動 Ctrl-Space / 自動トリガ共通）。
    /// 世代番号で連打時の古い応答を捨てる。
    fn request_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let Some((path, line, character, position)) = info else {
            return;
        };
        self.completion_generation = self.completion_generation.wrapping_add(1);
        let generation = self.completion_generation;
        let Some(lsp) = &self.lsp else {
            return;
        };
        let receiver = lsp.completion(&path, line, character);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, window, cx| {
                if workspace.completion_generation == generation {
                    workspace.show_completion(&value, position, window, cx)
                }
            });
        })
        .detach();
    }

    fn show_completion(
        &mut self,
        value: &serde_json::Value,
        position: Option<Point<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = parse_completion_items(value);
        if items.is_empty() {
            return;
        }
        // 絞り込みプレフィクスとポップアップ位置は**応答時点**のキャレットを使う
        // （要求中に打たれた分・直近 paint のキャレット矩形が反映される）。要求時点の位置は fallback。
        let (prefix, fresh_position) = self
            .active_editor()
            .map(|editor| {
                let view = editor.read(cx);
                (view.identifier_prefix_at_caret().1, view.caret_window_position())
            })
            .unwrap_or_default();
        let focus = cx.focus_handle();
        let position = fresh_position
            .or(position)
            .unwrap_or_else(|| point(px(220.), px(180.)));
        let state = CompletionState { items, prefix, selected: 0, position, focus };
        // 応答時点のプレフィクスで 1 件も残らなければ出さない。
        if state.filtered().is_empty() {
            return;
        }
        window.focus(&state.focus, cx);
        self.completion = Some(state);
        cx.notify();
    }

    fn close_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.completion.take().is_none() {
            return;
        }
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn move_completion_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(state) = self.completion.as_mut() {
            let len = state.filtered().len() as isize;
            if len == 0 {
                return;
            }
            state.selected = (state.selected as isize + delta).rem_euclid(len) as usize;
            cx.notify();
        }
    }

    fn confirm_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let insert = self.completion.as_ref().and_then(|state| {
            let filtered = state.filtered();
            filtered
                .get(state.selected)
                .and_then(|&index| state.items.get(index))
                .map(|item| item.insert_text.clone())
        });
        self.completion = None;
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            if let Some(text) = insert {
                editor.update(cx, |view, cx| view.apply_completion(&text, cx));
            }
        }
        cx.notify();
    }

    fn on_completion_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                // Esc = 同じ語の入力継続では自動再表示しない（語頭 offset を記憶）。
                self.completion_suppressed_word = self
                    .active_editor()
                    .map(|editor| editor.read(cx).identifier_prefix_at_caret().0);
                self.close_completion(window, cx);
            }
            "up" => self.move_completion_selection(-1, cx),
            "down" => self.move_completion_selection(1, cx),
            "enter" | "tab" => self.confirm_completion(window, cx),
            "backspace" => {
                // プレフィクスを 1 文字戻してエディタにも反映（空になったら閉じる）。
                if let Some(editor) = self.active_editor() {
                    editor.update(cx, |view, cx| view.delete_backward_char(cx));
                }
                let emptied = match self.completion.as_mut() {
                    Some(state) if !state.prefix.is_empty() => {
                        state.prefix.pop();
                        state.selected = 0;
                        state.filtered().is_empty()
                    }
                    _ => true,
                };
                if emptied {
                    self.close_completion(window, cx);
                }
                cx.notify();
            }
            _ => {
                // 印字キーは type-through: エディタへ挿入し、Typed イベント経由で
                // 絞り込み継続 / 新規トリガ / クローズが決まる（on_editor_typed）。
                let modifiers = event.keystroke.modifiers;
                let printable = !(modifiers.platform || modifiers.control || modifiers.function);
                let text = event
                    .keystroke
                    .key_char
                    .clone()
                    .filter(|text| printable && !text.is_empty() && !text.chars().any(char::is_control));
                match (text, self.active_editor()) {
                    (Some(text), Some(editor)) => {
                        editor.update(cx, |view, cx| view.insert_text(&text, cx));
                    }
                    // 印字以外（矢印+修飾・⌘系 等）は従来どおり閉じてエディタへ戻す。
                    _ => self.close_completion(window, cx),
                }
            }
        }
    }

    // ── フォーマット（⌥⇧F / 保存時・M11） ──

    fn format_document(&mut self, _: &Format, window: &mut Window, cx: &mut Context<Self>) {
        self.request_format(false, window, cx);
    }

    /// LSP フォーマットを要求して適用する。`save_after` = 適用後に保存（保存時フォーマット経路）。
    /// LSP が使えない/対応外のときは、`save_after` なら素の保存だけ行う。
    fn request_format(&mut self, save_after: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let Some(editor) = self.active_editor() else {
            return;
        };
        let formattable = {
            let view = editor.read(cx);
            view.buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .map(Path::to_path_buf)
        };
        let (Some(path), true) = (formattable, self.lsp_initialized) else {
            if save_after {
                editor.update(cx, |view, cx| view.save_now(cx));
            }
            return;
        };
        let Some(lsp) = &self.lsp else {
            if save_after {
                editor.update(cx, |view, cx| view.save_now(cx));
            }
            return;
        };
        let tab_size = settings::get(cx).tab_size as u32;
        let receiver = lsp.formatting(&path, tab_size);
        cx.spawn(async move |_workspace, cx| {
            let response = receiver.await;
            let _ = handle.update(cx, |workspace, _window, cx| {
                let Some(editor) = workspace.active_editor() else {
                    return;
                };
                // 応答が来た時に別ファイルへ移っていたら適用しない（誤爆防止）。
                if editor.read(cx).buffer().path() != Some(path.as_path()) {
                    return;
                }
                if let Ok(Ok(value)) = response {
                    let edits = parse_text_edits(&value);
                    if !edits.is_empty() {
                        editor.update(cx, |view, cx| {
                            let byte_edits = edits
                                .iter()
                                .map(|edit| {
                                    (
                                        view.lsp_range_to_bytes(
                                            edit.range.start.line,
                                            edit.range.start.character,
                                            edit.range.end.line,
                                            edit.range.end.character,
                                        ),
                                        edit.new_text.clone(),
                                    )
                                })
                                .collect();
                            view.apply_lsp_edits(byte_edits, cx);
                        });
                    }
                }
                if save_after {
                    editor.update(cx, |view, cx| view.save_now(cx));
                }
            });
        })
        .detach();
    }

    /// ⌘S。`format_on_save` が有効ならフォーマット → 保存、無効なら即保存（どちらも背景書き込み）。
    fn save_active(&mut self, _: &SaveActive, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        if settings::get(cx).format_on_save {
            self.request_format(true, window, cx);
        } else {
            editor.update(cx, |view, cx| view.save_now(cx));
        }
    }

    // ── rename（F2・M11） ──

    fn open_rename(&mut self, _: &Rename, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        // LSP 対応ファイルのみ。初期値はキャレット下の単語。
        let seed = {
            let view = editor.read(cx);
            if view
                .buffer()
                .path()
                .filter(|path| language_server_for(path, view.buffer().host().is_remote()).is_some())
                .is_none()
                || !self.lsp_initialized
            {
                return;
            }
            let snapshot = view.buffer().snapshot();
            let head = view.buffer().selections().first().map(|s| s.head).unwrap_or(0);
            snapshot
                .word_range_at(head)
                .map(|range| view.buffer().text_range(range))
                .unwrap_or_default()
        };
        if seed.is_empty() {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.rename_input = Some((seed, focus));
        cx.notify();
    }

    fn close_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename_input.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    fn on_rename_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_rename(window, cx),
            "enter" => {
                let new_name = self
                    .rename_input
                    .as_ref()
                    .map(|(value, _)| value.trim().to_string())
                    .unwrap_or_default();
                self.close_rename(window, cx);
                if !new_name.is_empty() {
                    self.perform_rename(new_name, window, cx);
                }
            }
            "backspace" => {
                if let Some((value, _)) = self.rename_input.as_mut() {
                    value.pop();
                    cx.notify();
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }
                if let Some((value, _)) = self.rename_input.as_mut() {
                    value.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    /// rename を LSP へ要求し、WorkspaceEdit を全ファイルへ適用する。
    /// 開いているタブはバッファへ（dirty のまま）・未オープンはディスクへ直書き。
    fn perform_rename(&mut self, new_name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let (path, line, character) = {
            let view = editor.read(cx);
            let Some(path) = view.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            let (line, character) = view.cursor_lsp_position();
            (path, line, character)
        };
        let Some(lsp) = &self.lsp else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let receiver = lsp.rename(&path, line, character, &new_name);
        cx.spawn(async move |_workspace, cx| {
            let Ok(Ok(value)) = receiver.await else {
                return;
            };
            let _ = handle.update(cx, |workspace, _window, cx| {
                workspace.apply_workspace_edit(&value, cx);
            });
        })
        .detach();
    }

    /// WorkspaceEdit を適用する共通経路（rename / code actions・M11）。
    fn apply_workspace_edit(&mut self, value: &serde_json::Value, cx: &mut Context<Self>) {
        let by_file = parse_workspace_edit(value);
        if by_file.is_empty() {
            return;
        }
        let mut buffers = 0usize;
        let mut disk_files = 0usize;
        let mut failed = 0usize;
        for file_edits in by_file {
            let path = file_edits.path;
            let edits = file_edits.edits;
            if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path) {
                let editor = tab.editor.clone();
                editor.update(cx, |view, cx| {
                    let byte_edits = edits
                        .iter()
                        .map(|edit| {
                            (
                                view.lsp_range_to_bytes(
                                    edit.range.start.line,
                                    edit.range.start.character,
                                    edit.range.end.line,
                                    edit.range.end.character,
                                ),
                                edit.new_text.clone(),
                            )
                        })
                        .collect();
                    view.apply_lsp_edits(byte_edits, cx);
                });
                buffers += 1;
                continue;
            }
            // 未オープン: ディスクへ直書き（読み → 適用 → 書き。UI スレッドだが local テキストで軽量。
            // remote は rename 要求自体が remote LSP 側なのでここへは来ない想定・来ても host 経由）。
            let Some(worktree) = self.active_worktree() else {
                continue;
            };
            let host = worktree.host().clone();
            match host.read_file(&path) {
                Ok(content) => match String::from_utf8(content.bytes) {
                    Ok(text) => {
                        let updated = apply_text_edits_to_string(&text, &edits);
                        match host.write_file(
                            &path,
                            updated.as_bytes(),
                            host::WriteCondition::Matches(content.revision),
                        ) {
                            Ok(_) => disk_files += 1,
                            Err(error) => {
                                failed += 1;
                                eprintln!("rename の書き込みに失敗 {}: {error:#}", path.display());
                            }
                        }
                    }
                    Err(_) => failed += 1,
                },
                Err(error) => {
                    failed += 1;
                    eprintln!("rename の読み込みに失敗 {}: {error:#}", path.display());
                }
            }
        }
        eprintln!("rename: バッファ {buffers}・ディスク {disk_files}・失敗 {failed}");
        self.refresh_git_status(cx);
        cx.notify();
    }

    /// F2 のミニ入力（キャレット近く…は座標が要るので v1 は中央上・⌃G と同型）。
    fn render_rename_input(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (value, focus) = self.rename_input.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let display: SharedString = SharedString::from(value.clone());
        Some(
            div()
                .absolute()
                .top(px(96.))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(320.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(30.))
                        .px(px(10.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(accent)
                        .rounded(px(8.))
                        .shadow(vec![gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4))
                            .blur_radius(px(16.))])
                        .track_focus(focus)
                        .on_key_down(cx.listener(Self::on_rename_key_down))
                        .text_size(px(12.5))
                        .text_color(theme.fg0)
                        .child(div().flex_none().text_size(px(11.)).text_color(theme.fg2).child(SharedString::from(i18n::t!("rename.label"))))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(display))
                        .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent)),
                )
                .into_any_element(),
        )
    }

    // ── ⌘I インライン編集（M12-8） ──

    // ⌘I: 選択範囲（無ければ現在行）を対象にインライン編集を開く。
    // 選択+指示 → `claude -p` で書き換え → その場 diff → accept/reject（チャットへ行かない最短経路）。
    // ターミナルにフォーカスがあれば同型の「自然言語 → コマンド生成」になる。
}
