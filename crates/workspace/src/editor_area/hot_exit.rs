impl Workspace {
    fn schedule_hot_exit_snapshot(&mut self, cx: &mut Context<Self>) {
        let debug = std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some();
        let Some(storage) = self.persistence.storage.clone() else {
            if debug {
                eprintln!("hotexit: storage=None でスキップ");
            }
            return;
        };
        self.hot_exit_gen = self.hot_exit_gen.wrapping_add(1);
        let generation = self.hot_exit_gen;
        if debug {
            eprintln!("hotexit: 予約 gen={generation}");
        }
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let snapshot: Vec<(PathBuf, String, bool)> = match workspace.update(cx, |workspace, cx| {
                if workspace.hot_exit_gen != generation {
                    return Vec::new(); // 後続の編集が来ている＝その回に任せる
                }
                if workspace.hot_exit_pending.is_some() {
                    // 復元/破棄が未決の間は書きも消しもしない（クリーン扱いで候補行を消してしまう）。
                    return Vec::new();
                }
                workspace
                    .tabs
                    .iter()
                    .map(|tab| {
                        let view = tab.editor.read(cx);
                        let dirty = view.buffer().is_dirty();
                        let text = if dirty { view.buffer().text() } else { String::new() };
                        (tab.path.clone(), text, dirty)
                    })
                    .collect()
            }) {
                Ok(snapshot) => snapshot,
                Err(_) => return,
            };
            if std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some() {
                eprintln!("hotexit: tick gen={generation} snapshot={} 件", snapshot.len());
            }
            if snapshot.is_empty() {
                return;
            }
            cx.background_executor()
                .spawn(async move {
                    for (path, text, dirty) in snapshot {
                        let result = if dirty {
                            storage.save_hot_exit(&path, &text)
                        } else {
                            storage.remove_hot_exit(&path)
                        };
                        if let Err(error) = result {
                            eprintln!("hot exit スナップショットに失敗: {error:#}");
                        }
                    }
                })
                .await;
        })
        .detach();
    }

    /// 起動時に前回の未保存スナップショットを探し、あれば復元/破棄バーを出す（main から呼ぶ）。
    pub fn check_hot_exit_restore(&mut self, cx: &mut Context<Self>) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        cx.spawn(async move |workspace, cx| {
            let rows = cx
                .background_executor()
                .spawn(async move { storage.load_hot_exit_all() })
                .await;
            let Ok(rows) = rows else { return };
            if rows.is_empty() {
                return;
            }
            if std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some() {
                eprintln!("hotexit: 復元候補 {} 件（バー表示）", rows.len());
            }
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.hot_exit_pending = Some(rows);
                cx.notify();
            });
        })
        .detach();
    }

    /// 復元バーの「復元」: スナップショットを各バッファへ流し込む（開いていなければ開く）。
    /// 置換は 1 Transaction なので undo で復元前に戻れる。復元後は dirty ＝次の tick で再スナップショット。
    fn restore_hot_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rows) = self.hot_exit_pending.take() else {
            return;
        };
        for (path, content) in rows {
            if !self.tabs.iter().any(|tab| tab.path == path) {
                self.open_file_sync(path.clone(), window, cx);
            }
            if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path) {
                let editor = tab.editor.clone();
                editor.update(cx, |view, cx| view.replace_all_text(&content, cx));
            } else {
                eprintln!("hot exit: 復元先を開けない（スキップ）: {}", path.display());
            }
        }
        cx.notify();
    }

    /// 復元バーの「破棄」: スナップショットを消してバーを閉じる。
    fn discard_hot_exit(&mut self, cx: &mut Context<Self>) {
        self.hot_exit_pending = None;
        if let Some(storage) = self.persistence.storage.clone() {
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.clear_hot_exit() {
                        eprintln!("hot exit の破棄に失敗: {error:#}");
                    }
                })
                .detach();
        }
        cx.notify();
    }

    /// 正常終了時の後始末（Quit アクションから）。スナップショットは破棄する（仕様: 正常終了で破棄）。
    pub fn prepare_quit(&mut self) {
        if let Some(storage) = &self.persistence.storage {
            if let Err(error) = storage.clear_hot_exit() {
                eprintln!("hot exit のクリアに失敗: {error:#}");
            }
        }
    }

    // ── ⌃G 行ジャンプ（M10-12） ──

    fn open_goto_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_editor().is_none() {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.goto_line = Some((String::new(), focus));
        cx.notify();
    }

    fn close_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.goto_line.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    fn on_goto_line_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_goto_line(window, cx),
            "enter" => {
                let line = self
                    .goto_line
                    .as_ref()
                    .and_then(|(value, _)| value.trim().parse::<usize>().ok());
                self.close_goto_line(window, cx);
                if let (Some(line), Some(editor)) = (line, self.active_editor()) {
                    self.record_nav_position(cx); // 行ジャンプもナビ履歴へ
                    editor.update(cx, |view, cx| {
                        view.reveal_position(line.saturating_sub(1), 0, cx)
                    });
                }
            }
            "backspace" => {
                if let Some((value, _)) = self.goto_line.as_mut() {
                    value.pop();
                    cx.notify();
                }
            }
            _ => {
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if text.chars().all(|c| c.is_ascii_digit()) && !text.is_empty() {
                    if let Some((value, _)) = self.goto_line.as_mut() {
                        if value.len() < 9 {
                            value.push_str(text);
                            cx.notify();
                        }
                    }
                }
            }
        }
    }

    /// ⌃G のミニ入力（中央上の小さな箱）。
    fn render_goto_line(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (value, focus) = self.goto_line.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let display: SharedString = if value.is_empty() {
            SharedString::from(i18n::t!("goto.placeholder"))
        } else {
            SharedString::from(value.clone())
        };
        let color = if value.is_empty() { theme.fg2 } else { theme.fg0 };
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
                        .w(px(220.))
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
                        .on_key_down(cx.listener(Self::on_goto_line_key_down))
                        .text_size(px(12.5))
                        .text_color(color)
                        .child(div().flex_none().text_color(theme.fg2).child(":"))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(display))
                        .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent)),
                )
                .into_any_element(),
        )
    }

    // ── ナビゲーション履歴（M10-11: ⌃- 戻る / ⌃⇧- 進む） ──

    /// 現在位置（アクティブタブのパス + キャレット byte offset）。
    fn current_nav_position(&self, cx: &App) -> Option<(PathBuf, usize)> {
        let editor = self.active_editor()?;
        let path = self.active_tab_path()?;
        let offset = editor.read(cx).buffer().selections().first().map(|s| s.head).unwrap_or(0);
        Some((path, offset))
    }

    /// ジャンプ級の移動の**直前**に呼ぶ: 現在位置を「戻る」へ積み、「進む」を捨てる。
    fn record_nav_position(&mut self, cx: &App) {
        let Some(position) = self.current_nav_position(cx) else {
            return;
        };
        if self.nav_back.last() == Some(&position) {
            return; // 連続同位置は積まない
        }
        self.nav_back.push(position);
        if self.nav_back.len() > 100 {
            self.nav_back.remove(0);
        }
        self.nav_forward.clear();
    }

    fn navigate_back(&mut self, _: &NavigateBack, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav_back.pop() else {
            return;
        };
        if let Some(current) = self.current_nav_position(cx) {
            self.nav_forward.push(current);
        }
        self.navigate_to(target, window, cx);
    }

    fn navigate_forward(&mut self, _: &NavigateForward, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav_forward.pop() else {
            return;
        };
        if let Some(current) = self.current_nav_position(cx) {
            self.nav_back.push(current);
        }
        self.navigate_to(target, window, cx);
    }

    /// 履歴の 1 点へ移動する（閉じたファイルは開き直す）。
    fn navigate_to(&mut self, target: (PathBuf, usize), window: &mut Window, cx: &mut Context<Self>) {
        let (path, offset) = target;
        if self.active_tab_path().as_ref() == Some(&path) {
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |view, cx| view.select_byte_range(offset..offset, cx));
            }
            return;
        }
        // 別ファイル: 開いて（開いていれば切替のみ）から着地する。open_file は背景読みなので
        // 完了後に reveal できるよう自前で合流する。
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |view, cx| view.select_byte_range(offset..offset, cx));
            }
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let host = worktree.host().clone();
        let read_path = path.clone();
        cx.spawn(async move |_workspace, cx| {
            let content = cx
                .background_executor()
                .spawn(async move { host.read_file(&read_path) })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| match content {
                Ok(content) => {
                    workspace.open_loaded_file(path, content, window, cx);
                    if let Some(editor) = workspace.active_editor() {
                        editor.update(cx, |view, cx| view.select_byte_range(offset..offset, cx));
                    }
                }
                Err(error) => eprintln!("履歴のファイルを開けない: {error:#}"),
            });
        })
        .detach();
    }

    // hover ポップアップを閉じる（タイプ・クリック・タブ切替・新しい dwell で呼ぶ）。
}
