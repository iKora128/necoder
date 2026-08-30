use crate::workspace::*;

// ── 開発用プローブ API（debug build 限定） ──
// NECODER_* 環境変数からオフスクリーン検証を駆動するための入口だけを置く。
// 全 item が `#[cfg(debug_assertions)]`。本番コードをここに置かない（release に混ざる）。

impl Workspace {
    /// 開発用: Agent タブの改名モーダルを開く（offscreen 検証・#4）。
    #[cfg(debug_assertions)]
    pub fn debug_tab_rename(&mut self, cx: &mut Context<Self>) {
        if !self.chrome.show_right {
            self.chrome.show_right = true;
        }
        self.agent_panel
            .update(cx, |panel, cx| panel.debug_start_rename(cx));
        cx.notify();
    }

    /// 開発用: スレッドに各状態を仕込んで開く（タブ/beacon/フッター/レールの状態表示を offscreen で検証・#）。
    #[cfg(debug_assertions)]
    /// 開発用: 擬似 tear-off を直接駆動（枠外ドロップ相当の座標 → 新窓生成まで・M13）。
    /// 実マウスの枠外リリースは自動化できないため、tear_off_rail_item の経路を機械検証する。
    #[cfg(debug_assertions)]
    pub fn debug_tear_off(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let before = cx.windows().len();
        let index = self.project_sessions.active;
        self.tear_off_rail_item(index, point(px(-120.), px(200.)), window, cx);
        println!("tearoff: windows {} -> {}", before, cx.windows().len());
    }

    pub fn debug_set_activities(&mut self, cx: &mut Context<Self>) {
        if !self.chrome.show_right {
            self.chrome.show_right = true;
        }
        self.agent_panel
            .update(cx, |panel, cx| panel.debug_set_activities(cx));
        // ニュースフィード（P2）の描画検証: 実遷移が積むのと同じ写像（news_text_for_phase）でデモ行を積む。
        if self.notifications.news.is_empty() {
            let statuses = self.agent_panel.read(cx).statuses();
            let demo = [
                (
                    TaskPhase::Blocked,
                    Some("workspace.rs への書き込みを許可しますか"),
                ),
                (
                    TaskPhase::ReviewReady,
                    Some("全 21 test green。次は P2 へ。"),
                ),
                (
                    TaskPhase::Failed,
                    Some("cargo check 2 エラー（notify 8.2 API 変更）"),
                ),
                (TaskPhase::Working, None),
            ];
            for (index, (phase, digest)) in demo.into_iter().enumerate() {
                let (kind, text) = Self::news_text_for_phase(phase, digest);
                let (color, title) = statuses
                    .get(index % statuses.len().max(1))
                    .map(|status| (status.color, status.name.clone()))
                    .unwrap_or((self.theme.fg2, SharedString::from("Task")));
                self.push_news(kind, color, title, text);
            }
        }
        cx.notify();
    }

    /// 開発用: スレッド履歴 Picker を開く（offscreen 検証・#5）。
    #[cfg(debug_assertions)]
    pub fn debug_open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_thread_history(&ThreadHistory, window, cx);
    }

    /// 開発用: 管制タブの受入検証（P3・`NECODER_CONTROL_PROBE`）。5 つの擬似 TaskSpace
    /// （Working/Blocked/MergeReady/Failed/Planned）を合成する。worktree 実体は現 root を共有
    /// （描画検証専用）・SpaceId は probe 専用値・storage は渡さない＝**Git/DB へ一切書かない**。
    #[cfg(debug_assertions)]
    pub fn debug_seed_control(&mut self, cx: &mut Context<Self>) {
        let Some(base) = self.project_sessions.projects.first() else {
            return;
        };
        let host = base.worktree.host().clone();
        let root = base.worktree.root().to_path_buf();
        let base_color = base.color;
        let scenarios: [(&str, &str, TaskPhase, u8, Option<&str>); 5] = [
            ("認証リファクタ", "task/auth", TaskPhase::Working, 0, None),
            ("バグ #412", "task/fix-412", TaskPhase::Blocked, 1, None),
            (
                "LP 文言",
                "task/lp-copy",
                TaskPhase::MergeReady,
                2,
                Some("Conflict Radar: clean — ヒーロー節 ja/en"),
            ),
            (
                "依存更新",
                "task/deps",
                TaskPhase::Failed,
                3,
                Some("cargo check 失敗 (2) — notify 8.2 の API 変更"),
            ),
            ("リサーチ", "task/research", TaskPhase::Planned, 4, None),
        ];
        for (title, branch, phase, style, summary) in scenarios {
            let Ok(worktree) = Worktree::with_host(host.clone(), &root) else {
                continue;
            };
            let color = base_color; // 色モデル: Task はリポジトリ色を継承（スレッド色が ACP を識別）
            let mut slot = ProjectSlot {
                task_space: TaskSpace::for_worktree(&worktree, Some(branch)),
                name: SharedString::from(title.to_string()),
                branch: Some(branch.to_string()),
                remote_host: None,
                color,
                worktree: Rc::new(worktree),
                explorer: ExplorerProject::default(),
                open_files: Vec::new(),
                active_file: 0,
                icon: None,
                icon_image: None,
                worktree_branch: Some(branch.to_string()),
            };
            slot.task_space.id = SpaceId(format!("probe-{branch}"));
            slot.task_space.title = SharedString::from(title.to_string());
            slot.task_space.phase = phase;
            slot.task_space.result_summary = summary.map(SharedString::from);
            let session = Self::create_project_session(
                Some(&slot),
                self.theme.clone(),
                self.explorer_mode(cx),
                None,
                cx,
            );
            let news_color = slot.color;
            self.project_sessions.projects.push(slot);
            self.project_sessions.sessions.push(session);
            let index = self.project_sessions.projects.len() - 1;
            self.project_sessions.sessions[index]
                .agent_panel
                .update(cx, |panel, cx| panel.debug_set_state(style, cx));
            // ニュースにも実遷移と同じ写像でデモ行を積む（Planned/Working は静かに）。
            if matches!(
                phase,
                TaskPhase::Blocked | TaskPhase::MergeReady | TaskPhase::Failed
            ) {
                let digest = match style {
                    1 => Some("shell: cargo publish を実行してよいですか"),
                    _ => summary,
                };
                let (kind, text) = Self::news_text_for_phase(phase, digest);
                self.push_news(
                    kind,
                    news_color,
                    SharedString::from(title.to_string()),
                    text,
                );
            }
        }
        self.chrome.fleet_mode = true;
        self.chrome.fleet_center_view = FleetCenterView::Control;
        // 監督バーの ✳ 総括の描画検証（実生成は oneshot・ここは見た目の確認用）。
        self.control_summary = Some(SharedString::from(
            "バグ #412 の publish 許可が最優先 — deps の失敗は独立、LP 文言は radar clean で統合可能",
        ));
        self.ensure_fleet_clock(cx);
        cx.notify();
    }

    /// 開発用: worktree 削除の確認ダイアログを出す（2026-07-27）。**実際に git を数えさせる**ので、
    /// 「未コミット N ファイル / 未統合 N 件」がその場のリポジトリの真値で描かれる。
    /// `branch` を渡すとブランチごと削除の見た目になる。削除自体は実行しない（人が押すまで）。
    #[cfg(debug_assertions)]
    pub fn debug_worktree_delete(
        &mut self,
        mode: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // `worktree:2` のようにレール index を指定できる（未指定はアクティブ）。
        // dirty / 未統合コミットがある slot を狙って「失うものがある」表示を撮るため。
        let (mode, index) = match mode.split_once(':') {
            Some((mode, index)) => (mode, index.parse().unwrap_or(self.project_sessions.active)),
            None => (mode, self.project_sessions.active),
        };
        // 対象が linked worktree でなくても導線を撮れるように、削除行の出現条件だけ満たしておく。
        if let Some(slot) = self.project_sessions.projects.get_mut(index) {
            if slot.worktree_branch.is_none() {
                slot.worktree_branch = Some("task/probe".to_string());
            }
        }
        self.request_worktree_delete(index, mode == "branch", window, cx);
    }

    /// 開発用: 編隊の片付け UI を offscreen で検証する（2026-07-27）。
    /// `menu` = セル 0 の ⋯ メニューを開く / `terminal` = 下段をターミナルタブへ /
    /// `tall` = 下段を高さ 320px（ドラッグ結果と同じ状態）/ `close-all` = 全セルを × して残数を出す。
    /// **実クリックの代わりに同じ入口を叩く**ので、経路（open → 実行）まで機械検証できる。
    #[cfg(debug_assertions)]
    pub fn debug_fleet_probe(
        &mut self,
        command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            "menu" => self.open_fleet_cell_menu(0, point(px(760.), px(210.)), cx),
            // セル 0 を拡大してヘッダのタイトルを改名開始（Cell site で入力欄が出る／herd と二重描画しない）。
            "rename" => {
                self.chrome.fleet_center_view = FleetCenterView::Graph;
                self.seed_fleet_cells(cx);
                if let Some(index) = self
                    .project_sessions
                    .projects
                    .iter()
                    .position(|slot| !slot.task_space.is_integration())
                {
                    self.chrome.fleet_maximized = Some(0);
                    self.start_task_rename(index, RenameSite::Cell, window, cx);
                }
            }
            // 中央をグリッド（系譜グラフ面）にして Task セルを seed する。CONTROL_PROBE の 5 slot が
            // セルヘッダで並ぶ（改名ダブルクリック・🗑 削除の描画検証用）。herd も左に出す。
            "graph" => {
                self.chrome.fleet_center_view = FleetCenterView::Graph;
                self.chrome.show_left = true;
                self.chrome.show_herd = true;
                self.seed_fleet_cells(cx);
                // 系譜グラフを畳んでグリッドを主に（各セルヘッダの改名/🗑 を大きく撮る）。
                self.chrome.graph_collapsed = true;
                cx.notify();
            }
            // セルを 1 枚拡大（セルヘッダを最大サイズで撮る＝改名ダブルクリック/🗑 の確認）。
            "maximize" => {
                self.chrome.fleet_center_view = FleetCenterView::Graph;
                self.seed_fleet_cells(cx);
                if !self.chrome.fleet_cells.is_empty() {
                    self.chrome.fleet_maximized = Some(0);
                }
                cx.notify();
            }
            "terminal" => self.set_fleet_bottom_view(FleetBottomView::Terminal, cx),
            "tall" => {
                self.chrome.bottom_height = 320.0;
                cx.notify();
            }
            // × を押した後にセルが復活しないこと（再シードの根治）を機械で確かめる。
            "close-all" => {
                while !self.chrome.fleet_cells.is_empty() {
                    self.close_fleet_cell(0, cx);
                }
                println!(
                    "fleet: cells after close-all = {}",
                    self.chrome.fleet_cells.len()
                );
            }
            other => eprintln!("FLEET_PROBE: 未知のコマンド {other}"),
        }
    }

    /// 開発用: AI 全画面（⌘⇧⏎）を駆動する。
    #[cfg(debug_assertions)]
    pub fn debug_agent_full_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_agent_full_screen(&ToggleAgentFullScreen, window, cx);
    }

    /// 開発用: SSH 入力バーを開く（M13 の描画検証）。
    #[cfg(debug_assertions)]
    pub fn debug_open_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ssh_input(window, cx);
    }

    /// 開発用: SSH ホストピッカーを開く（NECODER_SSH_HOST_PROBE の描画検証・M13）。
    #[cfg(debug_assertions)]
    pub fn debug_open_ssh_host_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ssh_host_picker(&RemoteSsh, window, cx);
    }

    /// 開発用: ターミナルを開いて file:line リンクのクリック相当イベントを発火（M13 の結線検証）。
    #[cfg(debug_assertions)]
    pub fn debug_terminal_link(
        &mut self,
        path: String,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_terminal(&ToggleTerminal, window, cx);
        self.terminal_dock
            .update(cx, |dock, cx| dock.emit_open_path(path, line, cx));
    }

    /// 開発用: ⌘O スイッチャーを開く（M12-12 のオフスクリーン検証）。
    #[cfg(debug_assertions)]
    pub fn debug_open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_project_switcher(&ProjectSwitcher, window, cx);
    }

    /// 開発用: ⌘P ファイルファインダを開く（`NECODER_FINDER_PROBE`・空プロジェクトの
    /// 作成アクション検証）。`confirm` = 列挙が終わって picker が開いた頃（700ms 後）に
    /// 先頭候補を確定する（空プロジェクトなら「新規ファイル…」→ 命名行が出るところまで通す）。
    #[cfg(debug_assertions)]
    pub fn debug_file_finder_probe(
        &mut self,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_finder(&FileFinder, window, cx);
        if !confirm {
            return;
        }
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        cx.spawn(async move |_workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(700))
                .await;
            let _ = handle.update(cx, |workspace, _window, cx| {
                if let Some(picker) = workspace.overlays.picker.clone() {
                    picker.update(cx, |picker, cx| picker.confirm_selected(cx));
                }
            });
        })
        .detach();
    }

    /// 開発用: エクスプローラ D&D の落着処理を直接叩く（`NECODER_DROP_PROBE`）。
    /// `"move:<src>:<dir>"` = 内部移動・`"copy:<絶対src>:<dir>"` = Finder からのコピー。
    /// src/dir はプロジェクト相対（copy の src は絶対）・dir 空 = ルート。
    /// ドラッグのジェスチャ（drag_over ハイライト）自体は実マウスでしか出ないので実機で見る。
    #[cfg(debug_assertions)]
    pub fn debug_drop_probe(&mut self, spec: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self
            .active_worktree()
            .map(|worktree| worktree.root().to_path_buf())
        else {
            return;
        };
        let dir_of = |dir: &str| {
            if dir.is_empty() {
                root.clone()
            } else {
                root.join(dir)
            }
        };
        let parts: Vec<&str> = spec.splitn(3, ':').collect();
        match parts[..] {
            ["move", source, dir] => {
                self.move_entry_by_drop(root.join(source), dir_of(dir), window, cx)
            }
            ["copy", source, dir] => {
                self.copy_external_paths_by_drop(&[PathBuf::from(source)], dir_of(dir), cx)
            }
            _ => eprintln!("NECODER_DROP_PROBE が不正: {spec:?}"),
        }
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
    /// `NECODER_TODOS_PLAN=1` なら ✨今日の計画も発火、`NECODER_TODOS_SEND=<line>` なら
    /// その行を ▶ で AI へ送る（受入「チェックがひとりでに入る」の自動 round trip）。
    #[cfg(debug_assertions)]
    pub fn debug_open_todo_board(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.todo_panel.read(cx).open {
            self.toggle_todo_board(&ToggleTodoBoard, window, cx);
        }
        if std::env::var("NECODER_TODOS_PLAN").is_ok_and(|value| value == "1") {
            self.run_daily_plan_for(self.project_sessions.active, cx);
        }
        if let Ok(line) = std::env::var("NECODER_TODOS_SEND") {
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
                                workspace.send_todo_to_agent_for(
                                    workspace.project_sessions.active,
                                    line,
                                    text,
                                    cx,
                                );
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

    /// 開発用: アクティブエディタが .md なら整形プレビュー（rendered）へ切り替える（⌘⇧V 相当）。
    #[cfg(debug_assertions)]
    pub fn debug_markdown_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.set_rendered_markdown(true, cx));
        }
    }

    /// 開発用: アクティブなローカル .html を OS 標準 WebView プレビューへ切り替える。
    #[cfg(debug_assertions)]
    pub fn debug_html_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.set_rendered_html(true, cx));
        }
    }

    /// 開発用: アクティブ .html の source ⇄ preview をトグルする（WebView 自動破棄の offscreen 検証）。
    #[cfg(debug_assertions)]
    pub fn debug_html_preview_toggle(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| {
                let on = !editor.rendered_html();
                editor.set_rendered_html(on, cx);
            });
        }
    }

    /// 開発用: 復元バーの「復元」を押す（オフスクリーン検証。pending が無ければ何もしない）。
    #[cfg(debug_assertions)]
    pub fn debug_restore_hot_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hot_exit_pending.is_some() {
            if std::env::var_os("NECODER_HOTEXIT_DEBUG").is_some() {
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
            self.remove_project_slot(self.project_sessions.active, window, cx);
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
}
