impl Workspace {
    fn explorer_mode(&self, cx: &App) -> ExplorerView {
        self.explorer.read(cx).view()
    }

    fn explorer_naming(&self, cx: &App) -> Option<ExplorerNaming> {
        self.explorer.read(cx).naming()
    }

    fn explorer_context_menu(&self, cx: &App) -> Option<ExplorerContextMenu> {
        self.explorer.read(cx).context_menu()
    }

    fn toggle_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.projects.get_mut(active) {
            if slot.explorer.expanded.contains(&path) {
                slot.explorer.expanded.remove(&path);
            } else {
                slot.explorer.expanded.insert(path);
            }
            slot.refresh();
            cx.notify();
        }
    }

    /// エクスプローラの表示モードを切り替える（左下スイッチャー）。
    /// カラム表示は幅が要るので、狭ければ広げる（以後ユーザーがドラッグで調整）。
    fn set_explorer_view(&mut self, view: ExplorerView, cx: &mut Context<Self>) {
        self.explorer.update(cx, |explorer, cx| explorer.set_view(view, cx));
        if view == ExplorerView::Columns && self.chrome.explorer_width < 440.0 {
            self.chrome.explorer_width = 440.0;
        }
        cx.notify();
    }

    /// カラム/アイコン表示で `dir` に入る（現在フォルダを更新）。ブレッドクラムの上位階層クリックでも使う。
    /// **ルート外へ出た場合**（隣のリポジトリへ辿る）は、ツリー表示だと current_dir を反映できないので
    /// カラム表示（Finder 風）へ自動で切り替える（M5 受入: マウスだけで上へ辿る）。
    fn enter_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let outside = self
            .active_slot()
            .map(|slot| !dir.starts_with(slot.worktree.root()))
            .unwrap_or(false);
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.projects.get_mut(active) {
            slot.explorer.current_dir = Some(dir.clone());
            slot.explorer.selected = Some(dir);
            slot.refresh();
        }
        if outside && self.explorer_mode(cx) == ExplorerView::Tree {
            self.explorer
                .update(cx, |explorer, cx| explorer.set_view(ExplorerView::Columns, cx));
        }
        cx.notify();
    }

    /// エクスプローラの右クリックメニューを出す（対象と位置を記録）。
    fn show_context_menu(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.explorer.update(cx, |explorer, cx| {
            explorer.show_context_menu(ExplorerContextMenu { path, is_dir, position }, cx)
        });
        cx.notify();
    }

    /// 右クリックメニューを閉じる（外側クリック・アクション実行後）。
    // ── ツリーのファイル操作（M10。local のみ・remote は M13 の Host 拡張と一緒に） ──

    /// インライン命名を開始する（新規ファイル/フォルダ = base の中 or 横・リネーム = target の名前）。
    fn start_naming(&mut self, kind: NamingKind, base: PathBuf, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (parent, target, initial) = match kind {
            NamingKind::Rename => {
                let parent = base.parent().map(Path::to_path_buf).unwrap_or_else(|| base.clone());
                let name = base.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                (parent, Some(base), name)
            }
            _ => {
                let parent = if is_dir {
                    base
                } else {
                    base.parent().map(Path::to_path_buf).unwrap_or(base)
                };
                (parent, None, String::new())
            }
        };
        // 親フォルダを展開しておく（入力行が見えるように）。
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.projects.get_mut(active) {
            slot.explorer.expanded.insert(parent.clone());
            slot.refresh();
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.explorer.update(cx, |explorer, cx| {
            explorer.set_naming(
                ExplorerNaming { kind, parent, target, value: initial, focus },
                cx,
            )
        });
        self.hide_context_menu(cx);
        cx.notify();
    }

    /// インライン命名の確定（Enter）。作成/リネームを実行してツリーを更新する。
    fn confirm_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(naming) = self
            .explorer
            .update(cx, |explorer, cx| explorer.take_naming(cx))
        else {
            return;
        };
        let name = naming.value.trim();
        if name.is_empty() || name.contains('/') {
            eprintln!("名前が不正: {name:?}");
            cx.notify();
            return;
        }
        let destination = naming.parent.join(name);
        let result = match naming.kind {
            NamingKind::NewFile => project::create_file_local(&destination),
            NamingKind::NewDir => project::create_dir_local(&destination),
            NamingKind::Rename => match &naming.target {
                Some(target) => {
                    // 開いているタブはリネーム前に閉じる（旧パスへの保存＝ファイル復活を防ぐ・v1）。
                    if let Some(index) = self.tabs.iter().position(|tab| &tab.path == target) {
                        self.close_tab_at(index, window, cx);
                    }
                    project::rename_local(target, &destination)
                }
                None => Ok(()),
            },
        };
        match result {
            Ok(()) => {
                if naming.kind == NamingKind::NewFile {
                    self.open_file(destination.clone(), window, cx);
                }
                let active = self.project_sessions.active;
                if let Some(slot) = self.project_sessions.projects.get_mut(active) {
                    slot.explorer.selected = Some(destination);
                    slot.refresh();
                }
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("ファイル操作に失敗: {error:#}"),
        }
        cx.notify();
    }

    /// インライン命名の中止（Esc・外側クリック）。
    fn cancel_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let naming = self
            .explorer
            .update(cx, |explorer, cx| explorer.take_naming(cx));
        if naming.is_some() {
            match self.active_editor() {
                Some(editor) => {
                    let handle = editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
                None => window.focus(&self.focus_handle, cx),
            }
            cx.notify();
        }
    }

    /// 命名入力のキー処理（検索パネルと同じ手書き流儀）。
    fn on_naming_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.cancel_naming(window, cx),
            "enter" => self.confirm_naming(window, cx),
            "backspace" => {
                self.explorer.update(cx, |explorer, cx| {
                    explorer.update_naming(|naming| {
                        naming.value.pop();
                    }, cx)
                });
            }
            "v" if event.keystroke.modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.explorer.update(cx, |explorer, cx| {
                        explorer.update_naming(|naming| naming.value.push_str(text.trim()), cx)
                    });
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
                self.explorer.update(cx, |explorer, cx| {
                    explorer.update_naming(|naming| naming.value.push_str(text), cx)
                });
            }
        }
    }

    /// 複製（`name copy.ext`）→ ツリー更新。
    fn duplicate_entry(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        match project::duplicate_local(&path) {
            Ok(copy) => {
                let active = self.project_sessions.active;
                if let Some(slot) = self.project_sessions.projects.get_mut(active) {
                    slot.explorer.selected = Some(copy);
                    slot.refresh();
                }
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("複製に失敗: {error:#}"),
        }
        cx.notify();
    }

    /// OS のゴミ箱へ（完全削除はしない）。開いているタブは先に閉じる。
    fn trash_entry(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.close_tab_at(index, window, cx);
        }
        match project::trash_local(&path) {
            Ok(()) => {
                let active = self.project_sessions.active;
                if let Some(slot) = self.project_sessions.projects.get_mut(active) {
                    if slot.explorer.selected.as_ref() == Some(&path) {
                        slot.explorer.selected = None;
                    }
                    slot.refresh();
                }
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("ゴミ箱に入れられない: {error:#}"),
        }
        cx.notify();
    }

    fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.hide_context_menu(cx));
    }

    /// フォルダを**新規ウィンドウ**でプロジェクトとして開く（ウィンドウモデルの核）。
    /// レール ＋: ネイティブのフォルダ選択ダイアログ → 選んだフォルダを**このウィンドウのレールへ追加**。
    fn add_project_via_dialog(&mut self, cx: &mut Context<Self>) {
        // 多重起動ガード: 既にダイアログが出ていれば無視（＋連打で Finder を何枚も開かない）。
        if self.overlays.add_project_dialog_open {
            return;
        }
        self.overlays.add_project_dialog_open = true;
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(i18n::t!("rail.add_prompt"))),
        });
        cx.spawn(async move |workspace, cx| {
            let result = receiver.await;
            // 成功・キャンセル・失敗の全経路でフラグを戻す（早期 return で戻し忘れない）。
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.overlays.add_project_dialog_open = false;
                if let Ok(Ok(Some(paths))) = result {
                    if let Some(path) = paths.into_iter().next() {
                        workspace.add_project_slot(path, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// フォルダをレールの新しいプロジェクト slot として足す（既にあれば切替のみ）。
    /// ＋ ダイアログ経由: ローカルフォルダをレールへ追加（既にあれば切替のみ）。
    fn add_project_slot(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.open_folder_in_rail(host::LocalHost::shared(), path, None, cx);
    }

    /// フォルダを**このウィンドウのレール**に開く（既にあれば切替のみ）。新窓は作らない
    /// — ブランチ/worktree の既定導線（新窓はレール右クリック→「新しいウィンドウで開く」の明示操作・M10-2）。
    /// `branch` を渡すと「リンク worktree タブ」として記録し、右クリックの worktree/ブランチ削除を出す。
    /// 同じリポジトリの別ブランチは identity 色が親と衝突しがち → 使用中でない色に倒して方向感覚を保つ。
    fn open_folder_in_rail(
        &mut self,
        host: Arc<dyn Host>,
        path: PathBuf,
        branch: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) =
            self.project_sessions.projects.iter().position(|slot| slot.worktree.root() == path.as_path())
        {
            self.overlays.pending_project_switch = Some(index);
            cx.notify();
            return;
        }
        let worktree = match Worktree::with_host(host, &path) {
            Ok(worktree) => worktree,
            Err(error) => {
                self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx);
                return;
            }
        };
        let identity = read_project_identity(worktree.root());
        // identity 色があっても既にレールで使われていれば、未使用のパレット色へ倒す。
        let color = match identity.0 {
            Some(color) if !self.color_in_use(color) => color,
            _ => self.next_free_color(),
        };
        let remote_host = worktree
            .host()
            .is_remote()
            .then(|| SharedString::from(worktree.host().display_name().to_string()));
        let mut slot = ProjectSlot {
            name: worktree.name().into(),
            branch: None,
            remote_host,
            color,
            worktree: Rc::new(worktree),
            explorer: ExplorerProject::default(),
            open_files: Vec::new(),
            active_file: 0,
            icon: identity.1,
            worktree_branch: branch,
        };
        slot.refresh();
        let session = Self::create_project_session(
            Some(&slot),
            self.theme.clone(),
            self.explorer_mode(cx),
            self.persistence.storage.clone(),
            cx,
        );
        let index = self.project_sessions.projects.len();
        self.project_sessions.projects.push(slot);
        if index == 0 {
            self.project_sessions.sessions[0] = session;
        } else {
            self.project_sessions.sessions.push(session);
        }
        // switch_project は window が要る（subscribe 経由に無い）ため、次の render で消化する。
        self.overlays.pending_project_switch = Some(index);
        cx.notify();
    }

    /// この色が既にレールのどれかのスロットで使われているか（色衝突の判定・小さな誤差を許容）。
    fn color_in_use(&self, color: Hsla) -> bool {
        self.project_sessions.projects.iter().any(|slot| colors_close(slot.color, color))
    }

    /// レールで未使用のパレット色（無ければスロット数で回す）。同色 2 枚を避けて方向感覚を保つ。
    fn next_free_color(&self) -> Hsla {
        (0..theme_core::IDENTITY_PALETTE_HEXES.len())
            .map(project_color)
            .find(|color| !self.color_in_use(*color))
            .unwrap_or_else(|| project_color(self.project_sessions.projects.len()))
    }

    // ── レール項目の右クリックメニュー（M10-2） ──

    /// レール項目の右クリックメニューを開く（色スウォッチ + 新規窓 / 外す / worktree・ブランチ削除）。
    fn open_rail_menu(&mut self, index: usize, position: Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.overlays.color_picker = None;
        self.overlays.rail_menu = Some(RailMenuState { project_index: index, position, confirm: None });
        cx.notify();
    }

    fn close_rail_menu(&mut self, cx: &mut Context<Self>) {
        if self.overlays.rail_menu.take().is_some() {
            cx.notify();
        }
    }

    /// スロットを**レールから外す**（表示のみ。ディスク・ブランチ・worktree は無傷＝安全側）。
    /// アクティブを外したら隣のスロットへビューを張り替える。最後の1枚は残す。
    fn remove_project_slot(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.overlays.rail_menu = None;
        if self.project_sessions.projects.len() <= 1 {
            self.push_toast(
                SharedString::from(i18n::t!("rail.cannot_remove_last")),
                self.accent(),
                cx,
            );
            cx.notify();
            return;
        }
        if index >= self.project_sessions.projects.len() {
            return;
        }
        let was_active = index == self.project_sessions.active;
        self.project_sessions.projects.remove(index);
        self.project_sessions.sessions.remove(index);
        // active index を詰める（後ろの要素が 1 つ前へずれる。ロジックは純関数でテスト済み）。
        self.project_sessions.active = active_index_after_removal(self.project_sessions.active, index, self.project_sessions.projects.len());
        if was_active {
            // アクティブを外した → 新しいアクティブスロットのビュー（タブ/LSP/端末/git/監視）へ張り替える。
            self.load_active_slot(window, cx);
        }
        self.save_state();
        cx.notify();
    }

    /// レール右クリック「worktree を削除」: `git worktree remove`（強制）→ スロットも外す。
    fn remove_slot_worktree(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_slot_worktree_impl(index, false, window, cx);
    }

    /// レール右クリック「worktree ごとブランチを削除」: worktree remove → `git branch -D` → スロットも外す。
    fn delete_slot_branch(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_slot_worktree_impl(index, true, window, cx);
    }

    /// worktree（+任意でブランチ）を消してレールから外す共通経路。背景で git を叩き完了後にスロットを外す。
    /// `git worktree remove` は対象ツリーの中からは実行できないため、メイン作業ツリーの dir で叩く。
    fn delete_slot_worktree_impl(
        &mut self,
        index: usize,
        also_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.rail_menu = None;
        // レール最後の1枚の worktree を消すと空レール＋ディスク破壊になる → 事前に断る（安全側）。
        if self.project_sessions.projects.len() <= 1 {
            self.push_toast(
                SharedString::from(i18n::t!("rail.cannot_remove_last")),
                self.accent(),
                cx,
            );
            cx.notify();
            return;
        }
        let Some(slot) = self.project_sessions.projects.get(index) else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let host = slot.worktree.host().clone();
        let target = slot.worktree.root().to_path_buf();
        let branch = slot.worktree_branch.clone();
        let Some(git_panel) = self.project_sessions.sessions.get(index).map(|session| session.git_panel.clone()) else {
            return;
        };
        git_panel.update(cx, |panel, cx| panel.set_busy(true, cx));
        let target_for_id = target.clone();
        cx.spawn(async move |_workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    // メイン作業ツリー（一覧の先頭 = 対象以外）の dir から remove を叩く。
                    let main = project::git_worktrees_on(host.as_ref(), &target)
                        .into_iter()
                        .map(|worktree| worktree.path)
                        .find(|path| *path != target)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "メインの作業ツリーは削除できません（レールから外すを使ってください）"
                            )
                        })?;
                    project::remove_worktree_on(host.as_ref(), &main, &target, true)?;
                    if also_branch {
                        if let Some(branch) = branch.as_deref() {
                            project::delete_branch_on(host.as_ref(), &main, branch, true)?;
                        }
                    }
                    Ok::<(Option<String>, bool), anyhow::Error>((branch, also_branch))
                })
                .await;
            let _ = handle.update(cx, |workspace, window, cx| {
                git_panel.update(cx, |panel, cx| panel.set_busy(false, cx));
                match result {
                    Ok((branch, also_branch)) => {
                        let message = if also_branch {
                            i18n::t!("git.branch_deleted", "branch" => branch.unwrap_or_default())
                        } else {
                            i18n::t!("git.worktree_removed")
                        };
                        workspace.push_toast(SharedString::from(message), workspace.accent(), cx);
                        if let Some(index) = workspace
                            .project_sessions
                            .projects
                            .iter()
                            .position(|slot| slot.worktree.root() == target_for_id.as_path())
                        {
                            workspace.remove_project_slot(index, window, cx);
                        }
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

    fn open_folder_as_window(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let source = match self.active_worktree() {
            Some(worktree) => match worktree.host().host_for_project(&path) {
                Ok(host) => ProjectSource::new(host, path),
                Err(error) => {
                    eprintln!("別 project を開けない: {error:#}");
                    return;
                }
            },
            None => ProjectSource::local(path),
        };
        self.open_source_as_window(source, cx);
        self.hide_context_menu(cx);
        cx.notify();
    }

    /// ProjectSource を新しいウィンドウで開く（ローカル folder / SSH の共通経路）。
    fn open_source_as_window(&mut self, source: ProjectSource, cx: &mut Context<Self>) {
        let theme = self.theme.clone();
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Shirushi".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(13.0), px(13.0))),
                }),
                is_movable: false,
                ..Default::default()
            },
            move |_window, cx| {
                cx.new(|cx| {
                    Workspace::new_sources(
                        vec![source.clone()],
                        theme.clone(),
                        state_path(),
                        cx,
                    )
                })
            },
        );
        if let Err(error) = opened {
            eprintln!("新規ウィンドウを開けない: {error}");
        }
    }

    /// パスをクリップボードへコピー。
    fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        self.hide_context_menu(cx);
        cx.notify();
    }

    /// Finder で表示（親フォルダを開いて選択・ローカルのみ）。
    fn reveal_in_finder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(error) = project::reveal_in_finder_local(path) {
            eprintln!("Finder 表示に失敗: {error:#}");
        }
        self.hide_context_menu(cx);
    }

    /// OS の既定アプリで開く（ファイル=関連付けアプリ / フォルダ=Finder・ローカルのみ）。
    fn open_with_default_app(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(error) = project::open_with_default_app_local(path) {
            eprintln!("既定アプリで開けない: {error:#}");
        }
        self.hide_context_menu(cx);
    }

    /// ファイルを開く（⌘P・ツリークリック・検索ジャンプ・F12 等の対話経路）。
    /// **読み込みは背景スレッド**（remote は 30s ブロックしうる — ARCHITECTURE §9）。
    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // 既に開いていれば重複タブを作らず、そのタブへ切り替える。
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
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
                Ok(content) => workspace.open_loaded_file(path, content, window, cx),
                Err(error) => eprintln!("ファイルを開けない: {error:#}"),
            });
        })
        .detach();
    }

    /// ファイルを**同期で**開く（起動復元・レール/ブランチ切替の `open_slot_files` 専用）。
    /// 対話経路は [`Self::open_file`]（背景読み込み）。同期版はタブの並び順を保つために残す。
    fn open_file_sync(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            return;
        }
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let content = match worktree.host().read_file(&path) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("ファイルを開けない: {error:#}");
                return;
            }
        };
        self.open_loaded_file(path, content, window, cx);
    }

    /// ファイルを開いて（既に開いていれば切替えて）、**開き終わったエディタ**へ `apply` を実行する。
    /// open_file は背景読みなので「開いてからジャンプ」はこの合流点を使う（旧バッファへの誤 reveal 防止）。
    fn open_file_then(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut EditorView, &mut Context<EditorView>) + 'static,
    ) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |view, cx| apply(view, cx));
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
                        editor.update(cx, |view, cx| apply(view, cx));
                    }
                }
                Err(error) => eprintln!("ファイルを開けない: {error:#}"),
            });
        })
        .detach();
    }

    /// 読み込み済み内容からタブを開く（open_file / open_file_sync の合流点）。
    fn open_loaded_file(
        &mut self,
        path: PathBuf,
        content: host::FileContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 読み込み中に同じファイルが開かれていたら切り替えるだけ。
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            return;
        }
        // 新しいタブがアクティブになる ＝ ⌘F バー・hover は畳む。
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let buffer = match Buffer::from_content(worktree.host().clone(), &path, content) {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("ファイルを開けない: {error:#}");
                return;
            }
        };
        let theme = self.theme.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let editor = cx.new(|cx| EditorView::new(buffer, theme, accent, cx));
        // settings の実効化（M10-13）: font_size/tab_size/soft_wrap を適用（live 変更は observe_global）。
        {
            let current = settings::get(cx);
            let soft_wrap =
                current.soft_wrap || std::env::var_os("SHIRUSHI_SOFT_WRAP").is_some();
            let (font_size, tab_size) = (current.font_size, current.tab_size);
            editor.update(cx, |view, cx| {
                view.set_typography(font_size, tab_size, cx);
                view.set_soft_wrap(soft_wrap, cx);
            });
        }

        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        // 変更を監視（再描画 + LSP didChange）+ 確定入力（補完の自動トリガ）+ hover dwell を購読。タブごとに持つ。
        let observation = cx.observe(&editor, Self::on_editor_changed);
        let input_subscription = cx.subscribe_in(&editor, window, Self::on_editor_typed);
        let hover_subscription = cx.subscribe_in(&editor, window, Self::on_editor_hover);
        self.tabs.push(EditorTab {
            path: path.clone(),
            editor,
            transient: false,
            _observation: observation,
            _input_subscription: input_subscription,
            _hover_subscription: hover_subscription,
        });
        self.active_tab = self.tabs.len() - 1;

        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.projects.get_mut(active) {
            slot.explorer.selected = Some(path.clone());
        }
        self.sync_active_slot();
        self.refresh_git_status(cx);
        // LSP: この拡張子にサーバがあれば起動 + didOpen（初期化済みなら即 didOpen）。
        let has_language_server = self
            .active_editor()
            .and_then(|editor| {
                let view = editor.read(cx);
                view.buffer().path().map(|path| {
                    language_server_for(path, view.buffer().host().is_remote()).is_some()
                })
            })
            .unwrap_or(false);
        if has_language_server {
            self.ensure_lsp(cx);
            if self.lsp_initialized {
                self.lsp_did_open_active(cx);
            }
            // 既知の診断があれば即反映。
            self.push_active_diagnostics(cx);
        }
        // 開発用: SHIRUSHI_SPLIT=1 で右分割ペインを開いた状態で撮る。
        if self.split_editor.is_none() && std::env::var_os("SHIRUSHI_SPLIT").is_some() {
            self.toggle_split(&SplitRight, window, cx);
        }
        self.save_state();
        cx.notify();
    }

    fn save_state(&self) {
        let Some(path) = self.persistence.state_path.as_ref() else {
            return;
        };
        let state = PersistedState {
            projects: self
                .project_sessions
                .projects
                .iter()
                .map(|slot| PersistedProject {
                    root: slot.worktree.root().to_path_buf(),
                    open_file: None, // 旧形式は書かない（open_files に一本化）
                    open_files: slot.open_files.clone(),
                    active_file: slot.active_file,
                    remote_uri: slot.worktree.host().project_uri(slot.worktree.root()),
                })
                .collect(),
            active: self.project_sessions.active,
        };
        let Ok(text) = serde_json::to_string_pretty(&state) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(path, text) {
            eprintln!("状態の保存に失敗: {error}");
        }
    }

    // ── オーバーレイ（Picker） ──

    // ⌘P ファイルファインダ。全ファイル列挙は背景（大リポジトリの walk / remote RPC で UI を止めない）。
}
