impl Workspace {
    fn on_terminal_dock_event(
        &mut self,
        dock: Entity<TerminalDock>,
        event: &TerminalDockEvent,
        cx: &mut Context<Self>,
    ) {
        let session_index = self
            .sessions
            .iter()
            .position(|session| session.terminal_dock == dock)
            .unwrap_or(self.active);
        match event {
            TerminalDockEvent::OpenPath { path, line } => {
                let resolved = if std::path::Path::new(path).is_absolute() {
                    PathBuf::from(path)
                } else if let Some(stripped) = path.strip_prefix("~/") {
                    std::env::home_dir()
                        .map(|home| home.join(stripped))
                        .unwrap_or_else(|| PathBuf::from(path))
                } else {
                    let Some(worktree) = self.projects.get(session_index).map(|slot| &slot.worktree)
                    else {
                        return;
                    };
                    worktree.root().join(path)
                };
                self.sessions[session_index].pending_navigation =
                    Some((resolved, line.saturating_sub(1) as usize, 0));
            }
            TerminalDockEvent::Dismissed if session_index == self.active => {
                self.chrome.show_bottom = false
            }
            TerminalDockEvent::Dismissed => {}
        }
        cx.notify();
    }

    // ── リモート SSH の GUI 導線（M13） ──

    /// titlebar の SSH ボタン: `ssh://user@host/path` の入力バーを開く（goto/rename と同型）。
    /// seed は「アクティブが remote ならその URI・そうでなければ ssh://」。
    #[cfg(debug_assertions)]
    fn open_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self
            .active_slot()
            .and_then(|slot| {
                let host = slot.worktree.host();
                host.is_remote().then(|| {
                    format!("{}{}", host.display_name(), slot.worktree.root().display())
                })
            })
            .map(|identity| {
                // display_name は "SSH user@host" 形式 → ssh://user@host に直す。
                identity.replace("SSH ", "ssh://").replace(" ", "")
            })
            .unwrap_or_else(|| "ssh://".to_string());
        self.open_ssh_input_seeded(seed, window, cx);
    }

    /// SSH 入力バーを種文字列付きで開く（ホストピッカーからの遷移でも使う）。
    fn open_ssh_input_seeded(&mut self, seed: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlays.ssh_connecting {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.overlays.ssh_input = Some((seed, focus));
        cx.notify();
    }

    /// リモート SSH ホストピッカー（M13）: `~/.ssh/config` の Host 一覧 + 末尾に「手入力」。
    /// 選択で `ssh://<alias>/` を種に入力バーへ（パスだけ足して Enter で接続）。
    /// system OpenSSH に委ねるので User/HostName/鍵/ProxyJump は config のものがそのまま効く。
    /// 開発用: Agent タブの改名入力を開く（offscreen 検証・#4）。
    #[cfg(debug_assertions)]
    pub fn debug_tab_rename(&mut self, cx: &mut Context<Self>) {
        if !self.chrome.show_right {
            self.chrome.show_right = true;
        }
        self.agent_panel.update(cx, |panel, cx| panel.debug_start_rename(cx));
        cx.notify();
    }

    /// 開発用: スレッド履歴 Picker を開く（offscreen 検証・#5）。
    #[cfg(debug_assertions)]
    pub fn debug_open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_thread_history(&ThreadHistory, window, cx);
    }

    /// スレッド履歴を開く（#5）。DB の全スレッド（アーカイブ含む・updated_at 降順）を Picker に出す。
    /// 行頭●= スレッド色・detail = プロジェクト / ⎇ branch / トークン累計。確定で復元してアクティブに。
    fn open_thread_history(&mut self, _: &ThreadHistory, window: &mut Window, cx: &mut Context<Self>) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        let threads = storage.load_all_threads().unwrap_or_default();
        let mut history = Vec::new();
        let mut items = Vec::new();
        for (id, name, color_index, project, branch, tokens_used, archived) in threads {
            let mut detail = String::new();
            if !project.is_empty() {
                detail.push_str(&project);
            }
            if let Some(branch) = &branch {
                detail.push_str(&format!("  ⎇ {branch}"));
            }
            if tokens_used > 0 {
                detail.push_str(&format!("  Σ {:.1}k", tokens_used as f32 / 1000.0));
            }
            if archived {
                detail.push_str("  ·閉");
            }
            let mut item = PickerItem::new(history.len(), name.clone())
                .with_accent(theme_core::thread_color(color_index as usize));
            if !detail.is_empty() {
                item = item.with_detail(detail);
            }
            items.push(item);
            history.push((id, name, color_index));
        }
        self.picker_history = history;
        self.open_picker(
            PickerMode::ThreadHistory,
            i18n::t!("agent.history_placeholder"),
            items,
            window,
            cx,
        );
    }

    fn open_ssh_host_picker(&mut self, _: &RemoteSsh, window: &mut Window, cx: &mut Context<Self>) {
        let hosts = host::ssh_config_hosts();
        // 2階層: 上=最近のリモートプロジェクト（履歴・直接接続・#5）、下=config ホスト、末尾=手入力。
        let recent = self
            .persistence
            .storage
            .as_ref()
            .and_then(|storage| storage.recent_remote_projects().ok())
            .unwrap_or_default();
        let mut items: Vec<PickerItem> = Vec::new();
        let mut recent_uris: Vec<String> = Vec::new();
        for (host_key, path, name, _opened_at) in recent.iter().take(20) {
            let id = recent_uris.len();
            items.push(
                PickerItem::new(id, name.clone())
                    .with_accent(self.accent()) // 行頭●= 最近のプロジェクトの目印
                    .with_detail(format!("{host_key}:{path}")),
            );
            recent_uris.push(format!("ssh://{host_key}{path}"));
        }
        let recent_count = recent_uris.len();
        // config ホスト（id = recent_count + index。選ぶと前回パス即接続 or パス入力）。
        for (offset, host) in hosts.iter().enumerate() {
            let mut item = PickerItem::new(recent_count + offset, host.alias.clone());
            let base = match (&host.user, &host.hostname) {
                (Some(user), Some(hostname)) => Some(format!("{user}@{hostname}")),
                (None, Some(hostname)) => Some(hostname.clone()),
                (Some(user), None) => Some(format!("{user}@{}", host.alias)),
                (None, None) => None,
            };
            // 前回パスがあれば併記（→ が即接続先・#2d）。
            let last_path = self
                .persistence
                .storage
                .as_ref()
                .and_then(|storage| storage.host_last_path(&host.alias).ok().flatten());
            let detail = match (base, last_path) {
                (Some(base), Some(path)) => Some(format!("{base}  →{path}")),
                (Some(base), None) => Some(base),
                (None, Some(path)) => Some(format!("→{path}")),
                (None, None) => None,
            };
            if let Some(detail) = detail {
                item = item.with_detail(detail);
            }
            items.push(item);
        }
        // 末尾は「手入力」= 生の ssh:// 入力バー（config に無いホストへの逃げ道）。
        items.push(
            PickerItem::new(recent_count + hosts.len(), i18n::t!("ssh.manual_entry"))
                .with_detail("ssh://user@host/path"),
        );
        self.picker_ssh_recent = recent_uris;
        self.picker_ssh_hosts = hosts;
        self.open_picker(
            PickerMode::SshHosts,
            i18n::t!("ssh.picker_placeholder"),
            items,
            window,
            cx,
        );
    }

    fn close_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlays.ssh_input.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
    }
}
    fn on_ssh_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_ssh_input(window, cx),
            "enter" => {
                let uri = self
                    .overlays
                    .ssh_input
                    .as_ref()
                    .map(|(value, _)| value.trim().to_string())
                    .unwrap_or_default();
                self.close_ssh_input(window, cx);
                if !uri.is_empty() && uri != "ssh://" {
                    self.connect_ssh_and_open(uri, cx);
                }
            }
            "backspace" => {
                if let Some((value, _)) = self.overlays.ssh_input.as_mut() {
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
                if let Some((value, _)) = self.overlays.ssh_input.as_mut() {
                    value.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    /// SSH 接続 →（成功したら）新しいウィンドウで開く。接続は ControlMaster + server 配備で
    /// 数秒〜かかるため背景で行い、失敗はトーストで返す。system OpenSSH に委ねるので
    /// ~/.ssh/config の Host エイリアス・鍵・ProxyJump・agent がそのまま効く。
    fn connect_ssh_and_open(&mut self, uri: String, cx: &mut Context<Self>) {
        self.overlays.ssh_connecting = true;
        self.push_toast(
            SharedString::from(i18n::t!("ssh.connecting", "uri" => uri.clone())),
            self.accent(),
            cx,
        );
        cx.notify();
        // 成功したらホスト別に前回パスを記録（#2d・次回は打たずに即接続）。
        let last_path = host::SshProject::parse(&uri)
            .ok()
            .map(|project| (project.host, project.path.to_string_lossy().to_string()));
        let storage = self.persistence.storage.clone();
        cx.spawn(async move |workspace, cx| {
            let source = cx
                .background_executor()
                .spawn(async move {
                    let project = host::SshProject::parse(&uri)?;
                    let server_command = std::env::var("SHIRUSHI_REMOTE_SERVER_COMMAND")
                        .unwrap_or_else(|_| "shirushi-remote-server".to_string());
                    let remote = host::RemoteHost::connect_ssh(&project, &server_command)?;
                    let root = remote.root().to_path_buf();
                    Ok::<ProjectSource, anyhow::Error>(ProjectSource::new(remote, root))
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.overlays.ssh_connecting = false;
                match source {
                    Ok(source) => {
                        if let (Some(storage), Some((host_key, path))) = (&storage, &last_path) {
                            let _ = storage.set_host_last_path(host_key, path);
                            // 履歴（最近のリモートプロジェクト）にも記録（#5・2階層ピッカー）。
                            // name = パス末尾のフォルダ名（無ければホスト名）。
                            let name = std::path::Path::new(path)
                                .file_name()
                                .map(|component| component.to_string_lossy().to_string())
                                .filter(|component| !component.is_empty())
                                .unwrap_or_else(|| host_key.clone());
                            let _ = storage.record_remote_project(host_key, path, &name);
                        }
                        workspace.open_source_as_window(source, cx);
                    }
                    Err(error) => workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// SSH 入力バー（rename/goto と同型の中央上オーバーレイ）。
    fn render_ssh_input(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (value, focus) = self.overlays.ssh_input.as_ref()?;
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
                        .w(px(460.))
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
                        .on_key_down(cx.listener(Self::on_ssh_key_down))
                        .text_size(px(12.5))
                        .text_color(theme.fg0)
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("ssh.label"))),
                        )
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(display))
                        .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent)),
                )
                .into_any_element(),
        )
    }

    // ── 自動アップデート（M13） ──

    /// 起動しばらく後に GitHub Releases を確認する（背景・失敗は静かに無視）。
    /// スクショ/プローブ実行時と `SHIRUSHI_NO_UPDATE_CHECK` ではネットへ出ない。
    fn schedule_update_check(&self, cx: &mut Context<Self>) {
        if std::env::var_os("SHIRUSHI_NO_UPDATE_CHECK").is_some()
            || std::env::var_os("SHIRUSHI_SCREENSHOT").is_some()
        {
            return;
        }
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(90))
                .await;
            let found = cx
                .background_executor()
                .spawn(async move { updater::check_for_update(env!("CARGO_PKG_VERSION")) })
                .await;
            if let Some(info) = found {
                let _ = workspace.update(cx, |workspace, cx| {
                    workspace.updater.status = Some((info, UpdateState::Available));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// statusbar チップのクリック: ダウンロード → 署名検証 → 差し替え（背景）。
    fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some((info, state)) = self.updater.status.clone() else {
            return;
        };
        if state != UpdateState::Available {
            return;
        }
        self.updater.status = Some((info.clone(), UpdateState::Installing));
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater::download_and_install(&info).map(|_| info) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match result {
                Ok(info) => {
                    workspace.updater.status = Some((info, UpdateState::Ready));
                    cx.notify();
                }
                Err(error) => {
                    workspace.updater.status = None;
                    workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// 開発用: SSH 入力バーを開く（M13 の描画検証）。
    #[cfg(debug_assertions)]
    pub fn debug_open_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ssh_input(window, cx);
    }

    /// 開発用: SSH ホストピッカーを開く（SHIRUSHI_SSH_HOST_PROBE の描画検証・M13）。
    #[cfg(debug_assertions)]
    pub fn debug_open_ssh_host_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ssh_host_picker(&RemoteSsh, window, cx);
    }

    /// 開発用: ターミナルを開いて file:line リンクのクリック相当イベントを発火（M13 の結線検証）。
    #[cfg(debug_assertions)]
    pub fn debug_terminal_link(&mut self, path: String, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_terminal(&ToggleTerminal, window, cx);
        self.terminal_dock
            .update(cx, |dock, cx| dock.emit_open_path(path, line, cx));
    }

    /// 開発用: ⌘O スイッチャーを開く（M12-12 のオフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_project_switcher(&ProjectSwitcher, window, cx);
    }

    /// 開発用: ⌘⇧P を開き、query で絞り込む（M13 のオフスクリーン検証）。
    /// `confirm` なら先頭候補を確定 = 実際にアクションが dispatch されるところまで通す。
    #[cfg(debug_assertions)]
    pub fn debug_palette_probe(
        &mut self,
        query: String,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_palette(&CommandPalette, window, cx);
        let Some(picker) = self.overlays.picker.clone() else {
            return;
        };
        picker.update(cx, |picker, cx| {
            if !query.is_empty() {
                picker.set_query(query, cx);
            }
            if confirm {
                picker.confirm_selected(cx);
            }
        });
    }

    /// 開発用: Todo ボードを開く（M12-10 のオフスクリーン検証）。
    /// `SHIRUSHI_TODOS_PLAN=1` なら ✨今日の計画も発火、`SHIRUSHI_TODOS_SEND=<line>` なら
    /// その行を ▶ で AI へ送る（受入「チェックがひとりでに入る」の自動 round trip）。
    #[cfg(debug_assertions)]
    pub fn debug_open_todo_board(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.todo_panel.read(cx).open {
            self.toggle_todo_board(&ToggleTodoBoard, window, cx);
        }
        if std::env::var("SHIRUSHI_TODOS_PLAN").is_ok_and(|value| value == "1") {
            self.run_daily_plan_for(self.active, cx);
        }
        if let Ok(line) = std::env::var("SHIRUSHI_TODOS_SEND") {
            if let Ok(line) = line.parse::<usize>() {
                // 板の読み込み（bg）完了を待ってから該当行を送る。
                let Some(handle) = window.window_handle().downcast::<Workspace>() else {
                    return;
                };
                cx.spawn(async move |_workspace, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    let _ = handle.update(cx, |workspace, _window, cx| {
                        let text = workspace
                            .todo_panel
                            .read(cx)
                            .items
                            .iter()
                            .find(|item| item.line == line)
                            .map(|item| item.text.clone());
                        match text {
                            Some(text) => {
                                eprintln!("TODOS_PROBE: ▶ 送信 line={line} text={text}");
                                workspace.send_todo_to_agent_for(workspace.active, line, text, cx);
                            }
                            None => eprintln!("TODOS_PROBE: line={line} が見つからない"),
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// 開発用: 全選択 → ⌘I → 指示を流し込んで実行（M12-8 のオフスクリーン検証）。
    /// `accept` なら提案到着をポーリングして適用 + 保存まで行う（受入の自動 round trip）。
    #[cfg(debug_assertions)]
    pub fn debug_inline_probe(
        &mut self,
        instruction: String,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |view, cx| {
            let end = view.buffer().len_bytes();
            view.select_byte_range(0..end, cx);
        });
        self.open_inline_edit(&InlineEdit, window, cx);
        let Some(state) = self.inline_edit.as_mut() else {
            eprintln!("INLINE_PROBE: 開けなかった（エディタ無し/対象空）");
            return;
        };
        state.instruction = instruction;
        self.execute_inline_edit(window, cx);
        if !accept {
            return;
        }
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            // 500ms 間隔で最大 120s（claude -p は数秒〜十数秒かかる）。
            for _ in 0..240 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let done = handle
                    .update(cx, |workspace, window, cx| {
                        let Some(state) = workspace.inline_edit.as_ref() else {
                            return true; // 閉じられた
                        };
                        if let Some(error) = &state.error {
                            eprintln!("INLINE_PROBE: 失敗 {error}");
                            return true;
                        }
                        if state.proposal.is_some() {
                            workspace.accept_inline_edit(window, cx);
                            workspace.save_active(&SaveActive, window, cx);
                            eprintln!("INLINE_PROBE: 適用+保存した");
                            return true;
                        }
                        false
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// 開発用: アクティブファイルの diff タブを開く（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_open_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_diff_tab(&OpenDiff, window, cx);
    }

    /// 開発用: ⌘⇧O アウトラインを開く（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_outline_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_outline(&OutlineSymbols, window, cx);
    }

    /// 開発用: ⌥⇧F 相当のフォーマット→保存を実行する（オフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_format_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_format(true, window, cx);
    }

    /// 開発用: 復元バーの「復元」を押す（オフスクリーン検証。pending が無ければ何もしない）。
    #[cfg(debug_assertions)]
    pub fn debug_restore_hot_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hot_exit_pending.is_some() {
            if std::env::var_os("SHIRUSHI_HOTEXIT_DEBUG").is_some() {
                eprintln!("hotexit: 自動復元を実行");
            }
            self.restore_hot_exit(window, cx);
        }
    }

    /// 開発用: (row, col) にキャレットを置いて ⌘K ⌘I 相当の hover を出す（オフスクリーン検証）。
    /// キャレット矩形は直近 paint 由来なので、移動後 1 拍おいてから hover を出す。
    #[cfg(debug_assertions)]
    pub fn debug_hover_probe(
        &mut self,
        row: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |view, cx| view.reveal_position(row, column, cx));
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                workspace.show_hover_at_caret(&ShowHover, window, cx);
            });
        })
        .detach();
    }

    /// 開発用: レール関連フローを実コードで駆動しオフスクリーン検証する（M10-2）。
    /// `open-branch:<name>` = ブランチを worktree としてレールに開く（新窓でなくレール）。
    /// `remove-active` = アクティブスロットをレールから外す（隣へビュー張り替えの確認）。
    #[cfg(debug_assertions)]
    pub fn debug_rail_probe(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(branch) = command.strip_prefix("open-branch:") {
            self.open_branch_worktree(branch.to_string(), cx);
        } else if command == "remove-active" {
            self.remove_project_slot(self.active, window, cx);
        }
    }

    /// 開発用: ⌘F バーをクエリ入りで開く（オフスクリーン検証）。`replace` があれば置換行も開く。
    #[cfg(debug_assertions)]
    pub fn debug_open_buffer_search(
        &mut self,
        query: String,
        replace: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_buffer_search_impl(replace.is_some(), window, cx);
        if let Some(state) = self.buffer_search.as_mut() {
            state.query = query;
            if let Some(replace) = replace {
                state.replace = replace;
            }
        }
        self.refresh_buffer_search(true, cx);
    }

    // ── 描画 ──

    // レールのアクティビティアイコン 1 個（Lucide SVG・単色で theme 色に着色）。クリックは呼び出し側で付ける。
}
