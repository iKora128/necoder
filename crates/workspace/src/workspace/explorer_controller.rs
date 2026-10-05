use crate::workspace::*;
use project::file_operations::{FileOperation, UndoError};

impl Workspace {
    pub(crate) fn explorer_mode(&self, cx: &App) -> ExplorerView {
        self.explorer.read(cx).view()
    }

    pub(crate) fn explorer_naming(&self, cx: &App) -> Option<ExplorerNaming> {
        self.explorer.read(cx).naming()
    }

    pub(crate) fn explorer_context_menu(&self, cx: &App) -> Option<ExplorerContextMenu> {
        self.explorer.read(cx).context_menu()
    }

    /// フォーカスをエクスプローラへ（⌘Z をファイル操作の取り消しにする・H30）。命名の入力中は
    /// 入力欄から奪わない（打ちかけの名前へのキーが消える）。
    pub(crate) fn focus_explorer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.explorer_naming(cx).is_some() {
            return;
        }
        let handle = self.chrome.explorer_focus.clone();
        window.focus(&handle, cx);
    }

    /// エクスプローラにフォーカスがある間のキー。Escape は開いているもの（破棄の確認・右クリック
    /// メニュー）を先に閉じ、何も無ければ作業面へ戻る（レールと同じ抜け口）。
    pub(crate) fn on_explorer_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key != "escape" || event.keystroke.modifiers.modified() {
            return;
        }
        if self.explorer_naming(cx).is_some() {
            return; // 命名は自分の Escape で取り消す（`on_naming_key_down`）
        }
        if self.explorer.read(cx).discard_confirm().is_some() {
            self.cancel_discard(cx);
        } else if self.explorer_context_menu(cx).is_some() {
            self.hide_context_menu(cx);
            cx.notify();
        } else {
            self.focus_session_surface(false, false, window, cx);
        }
        cx.stop_propagation();
    }

    /// 1 回のユーザー操作ぶんのファイル操作を、アクティブ project の取り消し履歴へ積む。
    pub(crate) fn record_file_operations(&mut self, operations: Vec<FileOperation>) {
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            slot.explorer.history.record(operations);
        }
    }

    /// ⌘Z（エクスプローラにフォーカスがある時だけ・H30）: 直前のファイル操作を 1 手戻す。
    /// 戻せない時（ゴミ箱に無い・戻し先に同名がある 等）は理由をトーストで出す。
    pub(crate) fn undo_file_operation(
        &mut self,
        _: &UndoFileOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 命名の入力中は何もしない（打ちかけの名前の裏でファイルが動くと驚く）。
        if self.explorer_naming(cx).is_some() {
            return;
        }
        let active = self.project_sessions.active;
        let Some(step) = self
            .project_sessions
            .slot_mut(active)
            .and_then(|slot| slot.explorer.history.pop())
        else {
            self.push_toast(
                SharedString::from(i18n::t!("explorer.undo_empty")),
                self.accent(),
                cx,
            );
            return;
        };
        // 戻すと消える・動くパス（作ったもの・動かした先）。開いているタブは先に閉じる
        // （旧パスへの保存でファイルが復活しないように・名前の変更と同じ流儀）。ただし未保存の
        // 変更があれば戻さない（取り消し 1 回で打った内容を失わせない）。
        let vanishing: Vec<PathBuf> = step
            .iter()
            .filter_map(|operation| match operation {
                FileOperation::Created { path } => Some(path.clone()),
                FileOperation::Moved { to, .. } => Some(to.clone()),
                FileOperation::Trashed { .. } => None,
            })
            .collect();
        let dirty = self.tabs.iter().find(|tab| {
            vanishing.iter().any(|path| tab.path.starts_with(path)) && tab.is_dirty(cx)
        });
        if let Some(tab) = dirty {
            let message = i18n::t!("explorer.undo_dirty", "name" => file_label(&tab.path));
            if let Some(slot) = self.project_sessions.slot_mut(active) {
                slot.explorer.history.record(step);
            }
            self.push_toast(SharedString::from(message), self.accent(), cx);
            return;
        }
        while let Some(index) = self
            .tabs
            .iter()
            .position(|tab| vanishing.iter().any(|path| tab.path.starts_with(path)))
        {
            self.close_tab_at(index, window, cx);
        }
        let result = project::file_operations::undo_file_operations_local(&step);
        let message = match &result {
            Ok(()) => undo_done_message(&step),
            Err(error) => {
                eprintln!("ファイル操作を取り消せない: {error}");
                undo_error_message(error)
            }
        };
        if result.is_ok() {
            // 戻った先を選択しておく（ツリーのどこに戻ったかが見える）。
            let restored = step.first().and_then(|operation| match operation {
                FileOperation::Moved { from, .. } => Some(from.clone()),
                FileOperation::Trashed { original, .. } => Some(original.clone()),
                FileOperation::Created { .. } => None,
            });
            if let Some(slot) = self.project_sessions.slot_mut(active) {
                slot.explorer.selected = restored;
            }
        }
        self.refresh_active_explorer(cx);
        self.refresh_git_status(cx);
        self.push_toast(SharedString::from(message), self.accent(), cx);
        self.focus_explorer(window, cx);
        cx.notify();
    }

    pub(crate) fn toggle_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            if slot.explorer.expanded.contains(&path) {
                slot.explorer.expanded.remove(&path);
            } else {
                slot.explorer.expanded.insert(path);
            }
            self.refresh_active_explorer(cx);
            cx.notify();
        }
    }

    /// アクティブ project のツリーを組み直す（[`Self::refresh_explorer_for`]）。
    pub(crate) fn refresh_active_explorer(&mut self, cx: &mut Context<Self>) {
        let active = self.project_sessions.active;
        self.refresh_explorer_for(active, cx);
    }

    /// project `index` のエクスプローラのツリーを組み直す。
    ///
    /// local は同期（一瞬で終わる・既存の挙動とテストをそのまま保つ）。remote は
    /// ディレクトリ 1 つにつき SSH の往復になるので読み取りを背景へ出し、手元のキャッシュで
    /// 行だけ先に組み直す（折り畳みと既読フォルダの展開は即座に映る）。以前はここが
    /// UI スレッドで往復しており、sleep 復帰直後にフォルダを開くと再接続を待って固まっていた。
    /// 完了時は宛先そのもの（host id + root）と世代で照合し、古い結果や別 project の結果は捨てる。
    /// root すら読めない（接続が落ちている）ときは手元の表示を残す。
    pub(crate) fn refresh_explorer_for(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(slot) = self.project_sessions.projects.get_mut(index) else {
            return;
        };
        if !slot.worktree.is_remote() {
            slot.refresh();
            return;
        }
        let root = slot.worktree.root().to_path_buf();
        slot.explorer.rebuild_rows(&root);
        let host = slot.worktree.host().clone();
        let host_id = host.id().to_string();
        let ignore = slot.worktree.ignore_snapshot();
        let directories = slot.explorer.directories_to_read(&root);
        let generation = slot.explorer.begin_refresh();
        let read_root = root.clone();
        cx.spawn(async move |workspace, cx| {
            let listings = cx
                .background_executor()
                .spawn(async move {
                    project::read_listings_on(host.as_ref(), &read_root, &ignore, &directories)
                })
                .await;
            let listings = match listings {
                Ok(listings) => listings,
                Err(error) => {
                    eprintln!("エクスプローラを更新できない: {error:#}");
                    return;
                }
            };
            let _ = workspace.update(cx, |workspace, cx| {
                let Some(slot) = workspace.project_sessions.projects.get_mut(index) else {
                    return;
                };
                let same_destination =
                    slot.worktree.root() == root && slot.worktree.host().id() == host_id;
                if !same_destination || !slot.explorer.is_latest_refresh(generation) {
                    return;
                }
                slot.explorer.apply_listings(&root, listings);
                cx.notify();
            });
        })
        .detach();
    }

    /// エクスプローラの表示モードを切り替える（左下スイッチャー）。
    /// カラム表示は幅が要るので、狭ければ広げる（以後ユーザーがドラッグで調整）。
    pub(crate) fn set_explorer_view(&mut self, view: ExplorerView, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.set_view(view, cx));
        if view == ExplorerView::Columns && self.chrome.explorer_width < 440.0 {
            self.chrome.explorer_width = 440.0;
        }
        cx.notify();
    }

    /// カラム/アイコン表示で `dir` に入る（現在フォルダを更新）。ブレッドクラムの上位階層クリックでも使う。
    /// **ルート外へ出た場合**（隣のリポジトリへ辿る）は、ツリー表示だと current_dir を反映できないので
    /// カラム表示（Finder 風）へ自動で切り替える（M5 受入: マウスだけで上へ辿る）。
    pub(crate) fn enter_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let outside = self
            .active_slot()
            .map(|slot| !dir.starts_with(slot.worktree.root()))
            .unwrap_or(false);
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            slot.explorer.current_dir = Some(dir.clone());
            slot.explorer.selected = Some(dir);
        }
        self.refresh_active_explorer(cx);
        if outside && self.explorer_mode(cx) == ExplorerView::Tree {
            self.explorer.update(cx, |explorer, cx| {
                explorer.set_view(ExplorerView::Columns, cx)
            });
        }
        cx.notify();
    }

    /// エクスプローラの右クリックメニューを出す（対象と位置を記録）。
    pub(crate) fn show_context_menu(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.explorer.update(cx, |explorer, cx| {
            explorer.show_context_menu(
                ExplorerContextMenu {
                    path,
                    is_dir,
                    background: false,
                    position,
                },
                cx,
            )
        });
        cx.notify();
    }

    /// 余白の右クリックのメニューを出す（`dir` = そのビューの文脈フォルダ）。
    pub(crate) fn show_background_menu(
        &mut self,
        dir: PathBuf,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.explorer.update(cx, |explorer, cx| {
            explorer.show_context_menu(
                ExplorerContextMenu {
                    path: dir,
                    is_dir: true,
                    background: true,
                    position,
                },
                cx,
            )
        });
        cx.notify();
    }

    /// 行・セルの右クリック。外側の余白も右クリックを受ける（[`Self::right_click_background`]）ので、
    /// 伝播を止めて行のメニューを余白のメニューで上書きさせない。止めるとドックのフォーカス移動
    /// （メニューの「ゴミ箱に入れる」→ ⌘Z で戻す・H30）も届かないので、ここで移す。
    pub(crate) fn right_click_entry(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.focus_explorer(window, cx);
        self.show_context_menu(path, is_dir, event.position, cx);
    }

    /// 余白（行の外）の右クリック = そのビューの文脈フォルダのメニュー。空のフォルダには行が
    /// 無いので、新規作成・Finder への入口はここだけになる（2026-10-05 本人要望）。
    pub(crate) fn right_click_background(
        &mut self,
        dir: PathBuf,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.focus_explorer(window, cx);
        self.show_background_menu(dir, event.position, cx);
    }

    /// 右クリックメニューを閉じる（外側クリック・アクション実行後）。
    // ── ツリーのファイル操作（M10。local のみ・remote は M13 の Host 拡張と一緒に） ──

    /// インライン命名を開始する（新規ファイル/フォルダ = base の中 or 横・リネーム = target の名前）。
    pub(crate) fn start_naming(
        &mut self,
        kind: NamingKind,
        base: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (parent, target, initial) = match kind {
            NamingKind::Rename => {
                let parent = base
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| base.clone());
                let name = base
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
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
        // 親フォルダを展開しておく（入力行が見えるように）。カラム/アイコンは見ているフォルダの中に
        // しか入力を出せないので、新規は作る場所へ入ってから名前を打つ（Finder の新規フォルダと同じ）。
        let enter_parent =
            kind != NamingKind::Rename && self.explorer_mode(cx) != ExplorerView::Tree;
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            slot.explorer.expanded.insert(parent.clone());
            if enter_parent {
                slot.explorer.current_dir = Some(parent.clone());
            }
        }
        self.refresh_active_explorer(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.explorer.update(cx, |explorer, cx| {
            explorer.set_naming(
                ExplorerNaming {
                    kind,
                    parent,
                    target,
                    value: initial,
                    focus,
                },
                cx,
            )
        });
        self.reveal_naming_row(cx);
        self.hide_context_menu(cx);
        cx.notify();
    }

    /// 命名の入力行をツリーの見える位置へ寄せる。ツリーは見えている行しか描かないので、
    /// 画面外のフォルダで「新規ファイル」を選ぶと入力行が描かれないまま打つことになる。
    fn reveal_naming_row(&mut self, cx: &mut Context<Self>) {
        let Some(naming) = self.explorer_naming(cx) else {
            return;
        };
        let Some(slot) = self.active_slot() else {
            return;
        };
        let display_rows = explorer::tree_display_rows(
            &slot.explorer.rows,
            Some(naming.placement()),
            slot.worktree.root(),
        );
        if let Some(index) = display_rows
            .iter()
            .position(|row| matches!(row, explorer::TreeDisplayRow::Naming { .. }))
        {
            self.chrome
                .explorer_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
        }
    }

    /// インライン命名の確定（Enter）。作成/リネームを実行してツリーを更新する。
    pub(crate) fn confirm_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let operation = match (naming.kind, naming.target) {
                    (NamingKind::Rename, Some(target)) => FileOperation::Moved {
                        from: target,
                        to: destination.clone(),
                    },
                    _ => FileOperation::Created {
                        path: destination.clone(),
                    },
                };
                self.record_file_operations(vec![operation]);
                if naming.kind == NamingKind::NewFile {
                    self.open_file(destination.clone(), window, cx);
                } else {
                    // 名前の変更・新規フォルダの直後に ⌘Z で戻せるよう、フォーカスはツリーに残す。
                    self.focus_explorer(window, cx);
                }
                let active = self.project_sessions.active;
                if let Some(slot) = self.project_sessions.slot_mut(active) {
                    slot.explorer.selected = Some(destination);
                }
                self.refresh_active_explorer(cx);
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("ファイル操作に失敗: {error:#}"),
        }
        cx.notify();
    }

    /// インライン命名の中止（Esc・外側クリック）。
    pub(crate) fn cancel_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(crate) fn on_naming_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => self.cancel_naming(window, cx),
            "enter" => self.confirm_naming(window, cx),
            "backspace" => {
                self.explorer.update(cx, |explorer, cx| {
                    explorer.update_naming(
                        |naming| {
                            naming.value.pop();
                        },
                        cx,
                    )
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

    /// エクスプローラ内 D&D 移動（Finder 風・M10 local のみ）: `source` を `target_dir` の中へ。
    /// 同じ親への drop・自分自身/配下への drop は no-op。上書きはしない（`rename_local` と同じ拒否）。
    pub(crate) fn move_entry_by_drop(
        &mut self,
        source: PathBuf,
        target_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target_dir.starts_with(&source) || source.parent() == Some(target_dir.as_path()) {
            return;
        }
        let Some(name) = source.file_name().map(|name| name.to_os_string()) else {
            return;
        };
        let destination = target_dir.join(name);
        // 上書き拒否はタブを閉じる**前**に（rename_local も拒否するが、先に閉じると失敗時にタブだけ失う）。
        if destination.exists() {
            self.push_toast(
                SharedString::from(i18n::t!("explorer.drop_exists")),
                self.accent(),
                cx,
            );
            return;
        }
        // 移動対象（フォルダなら配下ごと）の開いているタブは先に閉じる（旧パスへの保存＝復活防止・rename と同じ流儀）。
        while let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.path.starts_with(&source))
        {
            self.close_tab_at(index, window, cx);
        }
        match project::rename_local(&source, &destination) {
            Ok(()) => {
                self.record_file_operations(vec![FileOperation::Moved {
                    from: source,
                    to: destination.clone(),
                }]);
                let active = self.project_sessions.active;
                if let Some(slot) = self.project_sessions.slot_mut(active) {
                    slot.explorer.selected = Some(destination);
                }
                self.refresh_active_explorer(cx);
                self.refresh_git_status(cx);
            }
            Err(error) => {
                self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx)
            }
        }
        cx.notify();
    }

    /// Finder からの D&D（`ExternalPaths`）: 落とされたパスを `target_dir` の中へ**コピー**する
    /// （元は動かさない安全側・VSCode と同じ）。既にそこに居るパスと、フォルダを自分の配下へ
    /// 落とす形は no-op。同名は上書きせず toast で断る（複数落とした場合は最初のエラーだけ出す）。
    pub(crate) fn copy_external_paths_by_drop(
        &mut self,
        sources: &[PathBuf],
        target_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let mut copied = Vec::new();
        let mut first_error = None;
        for source in sources {
            if source.parent() == Some(target_dir.as_path()) || target_dir.starts_with(source) {
                continue;
            }
            match project::copy_into_local(source, &target_dir) {
                Ok(destination) => copied.push(destination),
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        // 1 回のドロップ = 1 手（⌘Z でまとめて戻る）。
        self.record_file_operations(
            copied
                .iter()
                .map(|path| FileOperation::Created { path: path.clone() })
                .collect(),
        );
        if let Some(destination) = copied.pop() {
            let active = self.project_sessions.active;
            if let Some(slot) = self.project_sessions.slot_mut(active) {
                slot.explorer.selected = Some(destination);
            }
            self.refresh_active_explorer(cx);
            self.refresh_git_status(cx);
        }
        if let Some(error) = first_error {
            self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx);
        }
        cx.notify();
    }

    /// 複製（`name copy.ext`）→ ツリー更新。
    pub(crate) fn duplicate_entry(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        match project::duplicate_local(&path) {
            Ok(copy) => {
                self.record_file_operations(vec![FileOperation::Created { path: copy.clone() }]);
                let active = self.project_sessions.active;
                if let Some(slot) = self.project_sessions.slot_mut(active) {
                    slot.explorer.selected = Some(copy);
                }
                self.refresh_active_explorer(cx);
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("複製に失敗: {error:#}"),
        }
        cx.notify();
    }

    /// OS のゴミ箱へ（完全削除はしない）。開いているタブは先に閉じる。
    pub(crate) fn trash_entry(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hide_context_menu(cx);
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.close_tab_at(index, window, cx);
        }
        match project::move_to_trash_local(&path) {
            Ok(trashed) => {
                // ゴミ箱の中の場所を覚えておく（⌘Z で戻す。分からなければ戻せないと出す）。
                self.record_file_operations(vec![FileOperation::Trashed {
                    original: path.clone(),
                    trashed,
                }]);
                let active = self.project_sessions.active;
                if let Some(slot) = self.project_sessions.slot_mut(active) {
                    if slot.explorer.selected.as_ref() == Some(&path) {
                        slot.explorer.selected = None;
                    }
                }
                self.refresh_active_explorer(cx);
                self.refresh_git_status(cx);
            }
            Err(error) => eprintln!("ゴミ箱に入れられない: {error:#}"),
        }
        cx.notify();
    }

    /// 右クリック「ステージ」（D16）: `git add -- <path>` を背景で。失敗はトーストで出す。
    pub(crate) fn stage_from_explorer(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { project::stage_path_on(host.as_ref(), &root, &path) })
                .await;
            let updated = workspace.update(cx, |workspace, cx| {
                if let Err(error) = result {
                    workspace.push_toast(
                        SharedString::from(format!("{error:#}")),
                        workspace.accent(),
                        cx,
                    );
                }
                workspace.refresh_git_status(cx);
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("ステージの後始末ができない: {error:#}");
            }
        })
        .detach();
    }

    /// 右クリック「変更を破棄…」: 取り消せない操作なので、確認を出してから行う。
    pub(crate) fn ask_discard(
        &mut self,
        path: PathBuf,
        status: StatusKind,
        cx: &mut Context<Self>,
    ) {
        self.hide_context_menu(cx);
        self.explorer.update(cx, |explorer, cx| {
            explorer.set_discard_confirm(Some(explorer::DiscardConfirm { path, status }), cx)
        });
        cx.notify();
    }

    pub(crate) fn cancel_discard(&mut self, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.set_discard_confirm(None, cx));
        cx.notify();
    }

    /// 確認の「破棄する」: HEAD の内容へ戻す（`git restore`・背景）。git に戻す先が無いファイル
    /// （未追跡・add しただけ）はゴミ箱へ入れ、エクスプローラの ⌘Z で戻せる形にしておく。
    pub(crate) fn confirm_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.explorer.read(cx).discard_confirm() else {
            return;
        };
        self.cancel_discard(cx);
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        let is_local = !worktree.is_remote();
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
        let path = confirm.path;
        cx.spawn(async move |_workspace, cx| {
            let git_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { project::discard_path_on(host.as_ref(), &root, &git_path) })
                .await;
            let updated = handle.update(cx, |workspace, window, cx| {
                workspace.finish_discard(path, is_local, result, window, cx)
            });
            if let Err(error) = updated {
                eprintln!("変更の破棄の後始末ができない: {error:#}");
            }
        })
        .detach();
    }

    fn finish_discard(
        &mut self,
        path: PathBuf,
        is_local: bool,
        result: anyhow::Result<project::DiscardOutcome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(project::DiscardOutcome::Restored) => {}
            Ok(project::DiscardOutcome::Untracked) if !is_local => self.push_toast(
                SharedString::from(i18n::t!("explorer.discard_untracked_remote")),
                self.accent(),
                cx,
            ),
            // add しただけで作業ツリーに無い（消した後）なら片付けるものが無い。
            Ok(project::DiscardOutcome::Untracked) if path.symlink_metadata().is_err() => {}
            Ok(project::DiscardOutcome::Untracked) => {
                while let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
                    self.close_tab_at(index, window, cx);
                }
                match project::move_to_trash_local(&path) {
                    Ok(trashed) => self.record_file_operations(vec![FileOperation::Trashed {
                        original: path.clone(),
                        trashed,
                    }]),
                    Err(error) => {
                        self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx)
                    }
                }
            }
            Err(error) => {
                self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx)
            }
        }
        self.refresh_active_explorer(cx);
        self.refresh_git_status(cx);
        cx.notify();
    }

    pub(crate) fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.hide_context_menu(cx));
    }

    /// フォルダを**新規ウィンドウ**でプロジェクトとして開く（ウィンドウモデルの核）。
    /// レール ＋: ネイティブのフォルダ選択ダイアログ → 選んだフォルダを**このウィンドウのレールへ追加**。
    pub(crate) fn add_project_via_dialog(&mut self, cx: &mut Context<Self>) {
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

    /// 「最近のプロジェクト」に記録する（「＋」統一オープンの「最近」用）。home（ブラウズ入口）は
    /// 残さない。local と remote で別テーブル（remote の host key は display_name から復元し、
    /// `connect_ssh_and_open` が記録するキー "user@host"/alias と一致させる）。
    pub(crate) fn record_recent_project(&self, host: &dyn Host, path: &Path, cx: &App) {
        // ローカルは OS の最近リストにも載せる（Dock 右クリック等・M13 メニューバー連携）。
        if !host.is_remote() {
            cx.add_recent_document(path);
        }
        let Some(storage) = self.persistence.storage.as_ref() else {
            return;
        };
        let path_str = path.to_string_lossy().to_string();
        if path_str.is_empty() || path_str == "/" {
            return; // home/ブラウズ入口は「最近」に残さない（home≠プロジェクト）
        }
        let name = path
            .file_name()
            .map(|component| component.to_string_lossy().to_string())
            .filter(|component| !component.is_empty())
            .unwrap_or_else(|| path_str.clone());
        if host.is_remote() {
            let host_key = host_scope_key(host);
            let _ = storage.record_remote_project(&host_key, &path_str, &name);
        } else {
            let _ = storage.record_local_project(&path_str, &name);
        }
    }

    /// フォルダをレールの新しいプロジェクト slot として足す（既にあれば切替のみ）。
    /// ＋ ダイアログ経由: ローカルフォルダをレールへ追加（既にあれば切替のみ）。「最近」にも記録。
    pub(crate) fn add_project_slot(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let host: Arc<dyn Host> = host::LocalHost::shared();
        self.record_recent_project(host.as_ref(), &path, cx);
        self.open_folder_in_rail(host, path, None, cx);
    }

    /// Finder「このアプリケーションで開く」/ Dock の最近リスト / Dock アイコンへの D&D から
    /// 届いたパスを開く（main の `cx.on_open_urls` → チャネル経由。CFBundleDocumentTypes とセット・M13）。
    /// フォルダ = レールへ追加して切替（`add_project_slot`）・ファイル = アクティブプロジェクトで開く。
    /// プロジェクト未オープンで単一ファイルが来たら、親フォルダをプロジェクトとして開いてから開く。
    pub fn open_external_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for path in paths {
            if path.is_dir() {
                self.add_project_slot(path, cx);
                continue;
            }
            if self.project_sessions.projects.is_empty() {
                if let Some(parent) = path.parent() {
                    self.add_project_slot(parent.to_path_buf(), cx);
                }
            }
            self.open_file_then(path, window, cx, |_, _| {});
        }
    }

    /// フォルダを**このウィンドウのレール**に開く（既にあれば切替のみ）。新窓は作らない
    /// — ブランチ/worktree の既定導線（新窓はレール右クリック→「新しいウィンドウで開く」の明示操作・M10-2）。
    /// `branch` を渡すと「リンク worktree タブ」として記録し、右クリックの worktree/ブランチ削除を出す。
    /// 同じリポジトリの別ブランチは identity 色が親と衝突しがち → 使用中でない色に倒して方向感覚を保つ。
    pub(crate) fn open_folder_in_rail(
        &mut self,
        host: Arc<dyn Host>,
        path: PathBuf,
        branch: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.switch_to_open_root(&path, cx) {
            return;
        }
        // local は往復が無いので同期（gitignore もその場で読む）。
        if !host.is_remote() {
            match Worktree::with_host(host, &path) {
                Ok(worktree) => {
                    let task_space = TaskSpace::for_worktree(&worktree, branch.as_deref());
                    self.add_worktree_to_rail(worktree, task_space, branch, cx);
                }
                Err(error) => {
                    self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx)
                }
            }
            return;
        }
        // remote は `with_host`（canonicalize / metadata / .gitignore）と git の問い合わせ
        // （repository_id / HEAD / ブランチ）で RTT×7 になる。再接続が要る場面ではその全部を
        // 待つことになるので背景へ。`Worktree` は Rc を持たないので background から返せる。
        let branch_for_task = branch.clone();
        cx.spawn(async move |workspace, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move {
                    let worktree = Worktree::with_host(host, &path)?;
                    let task_space = TaskSpace::for_worktree(&worktree, branch_for_task.as_deref());
                    Ok::<_, anyhow::Error>((worktree, task_space))
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match opened {
                Ok((worktree, task_space)) => {
                    // 読んでいる間に同じ root が別経路で載っていたら切替だけ。
                    if workspace.switch_to_open_root(worktree.root(), cx) {
                        return;
                    }
                    workspace.add_worktree_to_rail(worktree, task_space, branch, cx);
                }
                Err(error) => workspace.push_toast(
                    SharedString::from(format!("{error:#}")),
                    workspace.accent(),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// `root` が既にレールにあれば切替を予約して true。
    fn switch_to_open_root(&mut self, root: &Path, cx: &mut Context<Self>) -> bool {
        let Some(index) = self
            .project_sessions
            .projects
            .iter()
            .position(|slot| slot.worktree.root() == root)
        else {
            return false;
        };
        self.overlays.pending_project_switch = Some(index);
        cx.notify();
        true
    }

    /// 開けた worktree をレールの slot にする（`open_folder_in_rail` の後半。host への
    /// 問い合わせは済んでいる前提＝ここは UI スレッドで完結する）。
    pub(super) fn add_worktree_to_rail(
        &mut self,
        worktree: Worktree,
        mut task_space_preview: TaskSpace,
        branch: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // Fleet の「取り込む」で開いた worktree は、ブランチ名に関係なく Task にする（O21）。
        let canonical =
            |path: &Path| paths::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let root = canonical(worktree.root());
        let adopted = self
            .chrome
            .adopt_as_task
            .iter()
            .find(|path| canonical(path) == root)
            .cloned();
        if let Some(path) = &adopted {
            self.chrome.adopt_as_task.remove(path);
            task_space_preview.kind = SpaceKind::Task;
            if task_space_preview.base_oid.is_none() {
                task_space_preview.base_oid = task_space_preview.head_oid.clone();
            }
        }
        // リモートの `.necoder` はリモート側にあるので読まない（同名のローカルパスを拾わない）。
        let identity = if worktree.is_remote() {
            ProjectIdentity::default()
        } else {
            read_project_identity(worktree.root())
        };
        // 色モデル（2026-07-24 ユーザー確定）: **workspace（リポジトリ）で 1 色・スレッド（ACP）で 1 色**。
        // Task worktree は親リポジトリの色を継承する（worktree ごとに色を変えない＝方向感覚を守る）。
        let inherited = (!task_space_preview.is_integration())
            .then(|| {
                self.project_sessions
                    .projects
                    .iter()
                    .find(|slot| {
                        slot.task_space.repository_id == task_space_preview.repository_id
                            && slot.task_space.is_integration()
                    })
                    .map(|slot| slot.color)
            })
            .flatten();
        // 優先順: 親の色（Task worktree）> `.necoder` > DB に焼いた色 > 未使用パレット色。
        let color = inherited.unwrap_or(match identity.color {
            Some(color) if !self.color_in_use(color) => color,
            _ => self
                .stored_project_color(&worktree)
                .unwrap_or_else(|| self.next_free_color()),
        });
        let remote_host = worktree
            .host()
            .is_remote()
            .then(|| SharedString::from(worktree.host().display_name().to_string()));
        let mut slot = ProjectSlot {
            task_space: task_space_preview,
            name: worktree.name().into(),
            branch: None,
            remote_host,
            color,
            identity_color: identity.color,
            worktree: Rc::new(worktree),
            explorer: ExplorerProject::default(),
            open_files: Vec::new(),
            active_file: 0,
            pinned_files: Vec::new(),
            icon: identity.icon,
            icon_image: identity.icon_image,
            worktree_branch: branch,
        };
        // remote のツリーは slot をレールに載せてから背景で読む（下の `refresh_explorer_for`）。
        let is_remote = slot.worktree.is_remote();
        if !is_remote {
            slot.refresh();
        }
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
        // 決めた色を DB へ焼く（次に開くときも同じ色・並び順に依存しない）。
        self.persist_project_color(index);
        if adopted.is_some() {
            // 台帳に Task として残す（再起動してもブランチ名の判定で統合先へ戻らない）。
            self.make_task_space(index, cx);
        }
        // レールの worktree が増えた = Fleet の worktree 一覧を読み直す（O21・作った直後の worktree を
        // 「消えています」と見間違えない）。
        self.forget_fleet_worktrees();
        self.update_agent_destination_for(index, cx);
        if is_remote {
            self.refresh_explorer_for(index, cx);
            self.ensure_connection_pumps(cx);
        }
        // switch_project は window が要る（subscribe 経由に無い）ため、次の render で消化する。
        self.overlays.pending_project_switch = Some(index);
        cx.notify();
    }

    /// この色が既にレールのどれかのスロットで使われているか（色衝突の判定・小さな誤差を許容）。
    pub(crate) fn color_in_use(&self, color: Hsla) -> bool {
        self.project_sessions
            .projects
            .iter()
            .any(|slot| colors_close(slot.color, color))
    }

    /// レールで未使用のパレット色（無ければスロット数で回す）。同色 2 枚を避けて方向感覚を保つ。
    pub(crate) fn next_free_color(&self) -> Hsla {
        (0..theme_core::IDENTITY_PALETTE_HEXES.len())
            .map(project_color)
            .find(|color| !self.color_in_use(*color))
            .unwrap_or_else(|| project_color(self.project_sessions.projects.len()))
    }

    // ── レール項目の右クリックメニュー（M10-2） ──

    /// レール項目の右クリックメニューを開く（色スウォッチ + 新規窓 / 外す / worktree・ブランチ削除）。
    pub(crate) fn open_rail_menu(
        &mut self,
        index: usize,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.overlays.color_picker = None;
        self.overlays.rail_menu = Some(RailMenuState {
            project_index: index,
            position,
        });
        cx.notify();
    }

    pub(crate) fn close_rail_menu(&mut self, cx: &mut Context<Self>) {
        if self.overlays.rail_menu.take().is_some() {
            cx.notify();
        }
    }

    /// スロットを**レールから外す**（表示のみ。ディスク・ブランチ・worktree は無傷＝安全側）。
    /// アクティブを外したら隣のスロットへビューを張り替える。最後の1枚は残す。
    pub(crate) fn remove_project_slot(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.rail_menu = None;
        // レールの worktree が減った = Fleet の worktree 一覧を読み直す（O21）。
        self.forget_fleet_worktrees();
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
        self.project_sessions.active = active_index_after_removal(
            self.project_sessions.active,
            index,
            self.project_sessions.projects.len(),
        );
        if was_active {
            // アクティブを外した → 新しいアクティブスロットのビュー（タブ/LSP/端末/git/監視）へ張り替える。
            self.load_active_slot(window, cx);
        }
        self.save_state(cx);
        cx.notify();
    }

    /// worktree（+任意でブランチ）を消してレールから外す共通経路。背景で git を叩き完了後にスロットを外す。
    /// `git worktree remove` は対象ツリーの中からは実行できないため、メイン作業ツリーの dir で叩く。
    pub(crate) fn delete_slot_worktree_impl(
        &mut self,
        index: usize,
        also_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.rail_menu = None;
        self.chrome.fleet_cell_menu = None;
        // 消す前にそこで走っているエージェントを止める（削除中に worktree へ書かれるのを防ぐ）。
        if let Some(session) = self.project_sessions.sessions.get(index) {
            for panel in &session.fleet_agents {
                panel.update(cx, |panel, cx| panel.cancel_all_turns(cx));
            }
        }
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
        let Some(git_panel) = self
            .project_sessions
            .sessions
            .get(index)
            .map(|session| session.git_panel.clone())
        else {
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
                            anyhow::anyhow!(i18n::t!("explorer.main_worktree_undeletable"))
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
                            // 消えた TaskSpace のセルは編隊グリッドからも外す（幽霊セルを残さない）。
                            let space = workspace.project_sessions.projects[index]
                                .task_space
                                .id
                                .clone();
                            workspace.remove_fleet_cells_for(&space);
                            // その Task の Captain の分解案の行も閉じる（「やり直す」で消した Task を作り直さない・R09）。
                            workspace.close_proposal_rows_for_task(&space, &target_for_id, cx);
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

    pub(crate) fn open_folder_as_window(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        cx.notify();
        let Some(host) = self
            .active_worktree()
            .map(|worktree| worktree.host().clone())
        else {
            self.open_source_as_window(ProjectSource::local(path), cx);
            return;
        };
        if !host.is_remote() {
            match host.host_for_project(&path) {
                Ok(host) => self.open_source_as_window(ProjectSource::new(host, path), cx),
                Err(error) => eprintln!("別 project を開けない: {error:#}"),
            }
            return;
        }
        // remote の `host_for_project` は OpenProject の往復。背景で開いてから新窓を出す。
        // 新窓は「復元」と同じ遅延経路（`ProjectSource::restored` → `hydrate_restored_projects`）
        // で組み立て、窓を出すのに接続を待たない。root はここで正規化しておく（復元の契約）。
        cx.spawn(async move |workspace, cx| {
            let source = cx
                .background_executor()
                .spawn(async move {
                    let host = host.host_for_project(&path)?;
                    let root = host.canonicalize(&path)?;
                    Ok::<_, anyhow::Error>(ProjectSource::restored(host, root))
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match source {
                Ok(source) => workspace.open_source_as_window(source, cx),
                Err(error) => workspace.push_toast(
                    SharedString::from(format!("{error:#}")),
                    workspace.accent(),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// ディレクトリを**現在のウィンドウのレール**にプロジェクトとして開く（browse 中の「ここを開く」）。
    /// remote の worktree から呼べば同じ SSH 接続を再利用（`host_for_project`・再接続なし）＝
    /// home に繋いで → ツリーを辿って → このフォルダを開く、の最後の一歩。新窓は作らない
    /// （新窓が要るなら「新しいウィンドウで開く」・open_folder_as_window との対）。「最近」にも記録。
    pub(crate) fn open_dir_in_rail(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        cx.notify();
        let current = self
            .active_worktree()
            .map(|worktree| worktree.host().clone())
            .unwrap_or_else(host::LocalHost::shared);
        if !current.is_remote() {
            match current.host_for_project(&path) {
                Ok(host) => {
                    self.record_recent_project(host.as_ref(), &path, cx);
                    self.open_folder_in_rail(host, path, None, cx);
                }
                Err(error) => {
                    self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx)
                }
            }
            return;
        }
        // remote の `host_for_project` は OpenProject の往復。背景で開いてからレールへ。
        cx.spawn(async move |workspace, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move { current.host_for_project(&path).map(|host| (host, path)) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| match opened {
                Ok((host, path)) => {
                    workspace.record_recent_project(host.as_ref(), &path, cx);
                    workspace.open_folder_in_rail(host, path, None, cx);
                }
                Err(error) => workspace.push_toast(
                    SharedString::from(format!("{error:#}")),
                    workspace.accent(),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// ProjectSource を新しいウィンドウで開く（ローカル folder / SSH の共通経路）。
    pub(crate) fn open_source_as_window(&mut self, source: ProjectSource, cx: &mut Context<Self>) {
        self.open_source_as_window_at(source, None, cx);
    }

    /// `origin` = 新窓のスクリーン座標（擬似 tear-off のドロップ位置・M13）。None は中央。
    pub(crate) fn open_source_as_window_at(
        &mut self,
        source: ProjectSource,
        origin: Option<gpui::Point<gpui::Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme.clone();
        let storage = self.persistence.storage.clone();
        let bounds = match origin {
            Some(origin) => Bounds::new(origin, size(px(1280.0), px(800.0))),
            None => Bounds::centered(None, size(px(1280.0), px(800.0)), cx),
        };
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("necoder".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(13.0), px(13.0))),
                }),
                is_movable: false,
                ..Default::default()
            },
            move |window, cx| {
                // 新窓は DB を共有し、自分の行（新しい窓 ID）だけを書く。閉じたら閉じ印。
                let persistence = Some(WindowPersistence {
                    window_id: storage
                        .as_ref()
                        .map(|_| crate::persistence::new_window_session_id()),
                    storage,
                });
                if let Some(persistence) = &persistence {
                    crate::persistence::install_window_close_hook(window, cx, persistence);
                }
                cx.new(|cx| {
                    Workspace::new_sources(vec![source.clone()], theme.clone(), persistence, cx)
                })
            },
        );
        if let Err(error) = opened {
            eprintln!("新規ウィンドウを開けない: {error}");
        }
    }

    /// パスをクリップボードへコピー。何も起きないように見えるので、コピーした物をトーストで返す。
    pub(crate) fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let text = path.display().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.hide_context_menu(cx);
        let color = self.accent();
        self.push_toast(
            i18n::t!("explorer.copied_path", "path" => text).into(),
            color,
            cx,
        );
    }

    /// アクティブ project のエクスプローラで `path` を選択にする（ツリーのフォルダを押した時）。
    pub(crate) fn select_explorer_entry(&mut self, path: PathBuf) {
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            slot.explorer.selected = Some(path);
        }
    }

    /// エクスプローラでファイルを押した（ツリー / カラム / アイコン共通）。1 回目はプレビュータブ、
    /// ダブルクリックは OS の既定のアプリ（Finder と同じ手癖・2026-09-30 本人要望。1 回目で necoder
    /// にも開いている）。既定のアプリはこの Mac の上にしか無いので、SSH 先のファイルのダブルクリックは
    /// 従来どおり普通のタブにする。
    pub(crate) fn click_explorer_file(
        &mut self,
        path: PathBuf,
        click_count: usize,
        is_local: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match explorer_file_click(click_count, is_local) {
            ExplorerFileClick::Preview => {
                self.chrome.explorer_clicked_file = Some(path.clone());
                self.open_file_preview(path, window, cx);
            }
            ExplorerFileClick::DefaultApp => self.open_with_default_app(&path, cx),
            ExplorerFileClick::Keep => self.open_file(path, window, cx),
        }
    }

    /// Finder で表示（親フォルダを開いて選択・ローカルのみ）。
    pub(crate) fn reveal_in_finder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(error) = project::reveal_in_finder_local(path) {
            eprintln!("Finder 表示に失敗: {error:#}");
        }
        self.hide_context_menu(cx);
    }

    /// OS の既定アプリで開く（ファイル=関連付けアプリ / フォルダ=Finder・ローカルのみ）。
    pub(crate) fn open_with_default_app(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(error) = project::open_with_default_app_local(path) {
            eprintln!("既定アプリで開けない: {error:#}");
        }
        self.hide_context_menu(cx);
    }

    /// 対話でファイルを開いた・選んだことを、アクティブ project の「最近開いた」の先頭へ（⌘P・D19）。
    pub(crate) fn note_recent_file(&mut self, path: &Path) {
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            slot.explorer.note_opened(path);
        }
    }

    /// ファイルを開く（⌘P・SSH 先のファイルのダブルクリック・検索ジャンプ・F12 等の対話経路）。
    /// **読み込みは背景スレッド**（remote は 30s ブロックしうる — ARCHITECTURE §9）。
    /// プレビュータブで開いていたら普通のタブにする（開き直した＝そのファイルを使う意思）。
    pub(crate) fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file_as(path, false, window, cx);
    }

    /// [`Self::open_file`] の本体。`preview` = プレビュータブで開く（エクスプローラの 1 回クリック・
    /// `open_file_preview`）。
    pub(crate) fn open_file_as(
        &mut self,
        path: PathBuf,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 対話でファイルを開いたら設定ホームは退き、AI 全画面も畳む（起動復元の open_file_sync には
        // 入れない＝未オンボーディング時に settings を出しっぱなしにするゲートを壊さないため）。
        // 全画面のままだと中央が Agent なので、開いたタブが画面に出ない。
        self.chrome.show_settings = false;
        self.exit_agent_full_screen(cx);
        // Web タブの鍵（URL）は Web タブとして開く（⌘⇧T の復元もファイルと同じこの道を通る）。
        if let Some(url) = web_tab_url(&path) {
            self.open_web_tab(url, window, cx);
            return;
        }
        // 内蔵の配信で開いていた HTML（鍵は `file://`）も Web タブに戻す。
        if let Some(file) = static_tab_file(&path) {
            self.open_static_web_tab(file, window, cx);
            return;
        }
        self.note_recent_file(&path);
        // 既に開いていれば重複タブを作らず、そのタブへ切り替える。
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            if !preview {
                self.keep_preview_tab(index, cx);
            }
            self.select_tab(index, window, cx);
            return;
        }
        if preview {
            self.pending_preview_tab = Some(path.clone());
        } else if self.pending_preview_tab.as_ref() == Some(&path) {
            // プレビューで読み込み中に普通に開き直された（⌘P・SSH 先のダブルクリック）＝普通のタブで開く。
            self.pending_preview_tab = None;
        }
        let Some(host) = self.active_host() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
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
    pub(crate) fn open_file_sync(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            return;
        }
        // 復元するタブ列の中の Web タブ（鍵が URL）。モードは触らずにタブだけ戻す。
        if let Some(url) = web_tab_url(&path) {
            self.show_web_tab(url, window, cx);
            return;
        }
        if let Some(file) = static_tab_file(&path) {
            self.show_static_web_tab(file, window, cx);
            return;
        }
        let Some(host) = self.active_host() else {
            return;
        };
        let content = match host.read_file(&path) {
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
    pub(crate) fn open_file_then(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut EditorView, &mut Context<EditorView>) + 'static,
    ) {
        self.note_recent_file(&path);
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |view, cx| apply(view, cx));
            }
            return;
        }
        let Some(host) = self.active_host() else {
            return;
        };
        let Some(handle) = window.window_handle().downcast::<Workspace>() else {
            return;
        };
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

    /// 表示専用タブ（画像 / PDF）を追加してアクティブにする。バッファも LSP も持たないので
    /// エディタタブの購読群は要らず、選択状態・git・永続化の更新だけを共通で済ませる。
    fn push_display_only_tab(
        &mut self,
        path: PathBuf,
        content: TabContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = EditorTab {
            path: path.clone(),
            content,
            transient: false,
            pinned: false,
            preview: false,
        };
        let handle = tab.focus_handle(cx);
        window.focus(&handle, cx);
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            slot.explorer.selected = Some(path);
        }
        self.sync_active_slot();
        self.refresh_git_status(cx);
        self.save_state(cx);
        cx.notify();
    }

    /// 読み込み済み内容からタブを開く（open_file / open_file_sync の合流点）。プレビューで開こうと
    /// していたファイルなら、新しいタブをプレビューにして前のプレビューを置き換える（O26）。
    pub(crate) fn open_loaded_file(
        &mut self,
        path: PathBuf,
        content: host::FileContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let as_preview = self.pending_preview_tab.as_ref() == Some(&path);
        if as_preview {
            self.pending_preview_tab = None;
        }
        let tab_count = self.tabs.len();
        self.open_loaded_tab(path.clone(), content, window, cx);
        if as_preview && self.tabs.len() == tab_count + 1 {
            self.settle_preview_tab(&path, window, cx);
        }
    }

    /// [`Self::open_loaded_file`] の本体（タブを 1 枚足してアクティブにする）。
    fn open_loaded_tab(
        &mut self,
        path: PathBuf,
        content: host::FileContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // ⌘P の「最近開いた」（D19）。対話で開いた分は `open_file` が先頭へ寄せ済みなので、
        // ここは起動時の復元・プロジェクト切替で開き直した分を古い側へ足すだけ。
        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
            slot.explorer.note_reopened(&path);
        }
        // 読み込み中に同じファイルが開かれていたら切り替えるだけ。
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.select_tab(index, window, cx);
            return;
        }
        // 新しいタブがアクティブになる ＝ ⌘F バー・hover は畳む。
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let Some(host) = self.active_host() else {
            return;
        };
        // 画像はテキストバッファを作らず画像タブとして開く（編集・保存・LSP は関与しない）。
        if image_view::is_image_path(&path) {
            let theme = self.theme.clone();
            let view = cx.new(|cx| ImageView::new(&path, content.bytes, theme, cx));
            self.push_display_only_tab(path, TabContent::Image(view), window, cx);
            return;
        }
        // PDF も同じ扱い。自前でデコードせず OS のビューア（macOS = WKWebView の PDFKit /
        // Windows = WebView2）に file:// を渡す。local は元のファイルを直接読ませるのでバイト列は
        // 捨てられ、remote だけがローカル複製の材料に使う（PdfView::new）。
        if pdf_view::is_pdf_path(&path) {
            let theme = self.theme.clone();
            let remote = host.is_remote();
            let view = cx.new(|cx| PdfView::new(&path, remote, content.bytes, theme, cx));
            let evict_minutes = settings::get(cx).html_preview_evict_minutes;
            view.update(cx, |view, cx| view.set_evict_minutes(evict_minutes, cx));
            self.push_display_only_tab(path, TabContent::Pdf(view), window, cx);
            return;
        }
        let buffer = match Buffer::from_content(host.clone(), &path, content) {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("ファイルを開けない: {error:#}");
                return;
            }
        };
        let theme = self.theme.clone();
        // プロジェクトではその色、Chat ではいま見ているチャットのスレッド色（`accent()`）。
        let accent = self.accent();
        let editor = cx.new(|cx| EditorView::new(buffer, theme, accent, cx));
        // チャットの `artifacts/` の中の HTML は**エージェントが書いた JavaScript**なので、どの経路で
        // 開いても閉じ込めて見せる（`file://` だとページから手元のファイルを読める）。タブ生成の
        // この一点で決めるので、`▣ プレビュー`・パスのリンク・⌘⇧V のどれから出しても同じ扱いになる。
        if let Some(root) = chat_core::folder::chats_root(Some(&settings::get(cx).chat.directory))
            .and_then(|chats| chat_core::folder::artifacts_root_of(&chats, &path))
        {
            editor.update(cx, |view, cx| view.sandbox_html_preview(root, cx));
        }
        // settings の実効化（M10-13）: font_size/tab_size/soft_wrap を適用（live 変更は observe_global）。
        {
            let current = settings::get(cx);
            let soft_wrap = current.soft_wrap || std::env::var_os("NECODER_SOFT_WRAP").is_some();
            let (font_size, tab_size) = (current.font_size, current.tab_size);
            editor.update(cx, |view, cx| {
                view.set_typography(font_size, tab_size, cx);
                view.set_soft_wrap(soft_wrap, cx);
                view.set_html_preview_evict_minutes(current.html_preview_evict_minutes, cx);
            });
        }

        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        // 変更を監視（再描画 + LSP didChange）+ 確定入力（補完の自動トリガ）+ hover dwell を購読。タブごとに持つ。
        let observation = cx.observe(&editor, Self::on_editor_changed);
        let input_subscription = cx.subscribe_in(&editor, window, Self::on_editor_typed);
        let hover_subscription = cx.subscribe_in(&editor, window, Self::on_editor_hover);
        let link_subscription = cx.subscribe_in(&editor, window, Self::on_preview_link);
        let blur_subscription = self.auto_save_on_blur(&editor, window, cx);
        self.tabs.push(EditorTab {
            path: path.clone(),
            content: TabContent::Editor {
                editor,
                _observation: observation,
                _input_subscription: input_subscription,
                _hover_subscription: hover_subscription,
                _link_subscription: link_subscription,
                _blur_subscription: blur_subscription,
            },
            transient: false,
            pinned: false,
            preview: false,
        });
        self.active_tab = self.tabs.len() - 1;

        let active = self.project_sessions.active;
        if let Some(slot) = self.project_sessions.slot_mut(active) {
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
        // 開発用: NECODER_SPLIT=1 で右分割ペインを開いた状態で撮る。
        if self.split_editor.is_none() && std::env::var_os("NECODER_SPLIT").is_some() {
            self.toggle_split(&SplitRight, window, cx);
        }
        self.save_state(cx);
        cx.notify();
    }

    /// エクスプローラの手動更新（ヘッダ右端の ↻）。監視（`handle_watch_events`）と同じ反映を
    /// アクティブプロジェクト全体に行う: ツリー再構築 + git 色 + アクティブエディタの gutter diff。
    pub(crate) fn refresh_explorer(&mut self, cx: &mut Context<Self>) {
        let active = self.project_sessions.active;
        // remote は読み取りを背景へ（同期で読むと接続が不調なとき ↻ を押すたびに固まる）。
        self.refresh_explorer_for(active, cx);
        self.refresh_git_status_for(active, cx);
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |view, cx| view.refresh_diff(cx));
        }
        cx.notify();
    }

    /// この窓の DB 上の行 ID（永続化しない窓は None）。⌘Q 時の「開いていた窓の集合」に使う。
    pub fn window_session_id(&self) -> Option<String> {
        self.persistence
            .session_writer
            .as_ref()
            .map(|writer| writer.window_id().to_string())
    }

    /// ユーザーがこの窓を閉じた印を DB の自分の行へ付ける（次回起動で復元しない）。
    /// OS 経由の閉じは `install_window_close_hook` が呼ぶ。自前 titlebar の × はこれを直接呼ぶ。
    ///
    /// 閉じ印を付けると以後この窓は書けなくなるので、**今のタブ列を書き切ってから**印を付ける
    /// （郵便受けに残っていた分を捨てると、直前に閉じたタブが次回起動で復活する）。
    pub(crate) fn mark_window_closed(&mut self) {
        self.sync_active_slot();
        let payload = encode_window_session(&self.persisted_state());
        if let Some(writer) = self.persistence.session_writer.as_ref() {
            writer.close(payload);
        }
    }

    /// この窓のセッション（プロジェクト列・各プロジェクトのタブ列）を DB の自分の行へ書く。
    /// 書き込みは `WindowSessionWriter` が background で合流する（UI スレッドで DB を待たない・順序保証）。
    pub(crate) fn save_state(&self, cx: &App) {
        let Some(writer) = self.persistence.session_writer.as_ref() else {
            return;
        };
        let Some(payload) = encode_window_session(&self.persisted_state()) else {
            return;
        };
        writer.save(payload, cx.background_executor());
    }

    pub(crate) fn persisted_state(&self) -> PersistedState {
        PersistedState {
            projects: self
                .project_sessions
                .projects
                .iter()
                .map(|slot| PersistedProject {
                    root: slot.worktree.root().to_path_buf(),
                    open_files: slot.open_files.clone(),
                    active_file: slot.active_file,
                    remote_uri: slot.worktree.host().project_uri(slot.worktree.root()),
                    pinned_files: slot.pinned_files.clone(),
                })
                .collect(),
            active: self.project_sessions.active,
            fleet_mode: self.chrome.fleet_mode,
            chat_mode: self.chat_mode(),
            chat_active: self.chrome.chat_shown.clone(),
            left_dock_width: self.chrome.explorer_width,
            stage_pinned: self
                .chrome
                .stage_pinned
                .iter()
                .map(|space| space.as_str().to_string())
                .collect(),
            stage_columns: self.chrome.stage_columns,
        }
    }

    /// 窓セッションの payload から「面」の状態（Fleet / Chat・左ドック幅）を戻す。プロジェクト列とタブ列は
    /// 起動時に別経路（`SavedProject`）で戻すので、ここは [`Self::persisted_state`] の残りの対。
    pub fn restore_window_state(&mut self, payload: &str, cx: &mut Context<Self>) {
        let Ok(saved) = serde_json::from_str::<PersistedState>(payload) else {
            return;
        };
        self.chrome.fleet_mode = saved.fleet_mode && !saved.chat_mode;
        // Chat へ入るには window が要る（フォーカスの付け直し）ので、次の描画で消化する。
        self.chrome.pending_chat_mode = saved.chat_mode;
        self.chrome.chat_restore = saved.chat_active;
        if saved.left_dock_width >= 160.0 {
            self.chrome.explorer_width = saved.left_dock_width.min(720.0);
        }
        self.chrome.show_herd |= saved.fleet_mode;
        // 舞台のピンと列数（O21）。消えた Task のピンは舞台に出る時に読み飛ばされる。
        self.chrome.stage_pinned = saved
            .stage_pinned
            .into_iter()
            .take(3)
            .map(SpaceId)
            .collect();
        if (1..=3).contains(&saved.stage_columns) {
            self.chrome.stage_columns = saved.stage_columns;
        }
        cx.notify();
    }

    /// 終了直前は background task に任せず、現時点の Workspace 列とタブ列を DB へ同期保存する。
    pub(crate) fn flush_window_session_for_quit(&mut self) {
        self.sync_active_slot();
        let Some(payload) = encode_window_session(&self.persisted_state()) else {
            return;
        };
        if let Some(writer) = self.persistence.session_writer.as_ref() {
            writer.flush_for_quit(payload);
        }
    }

    // ── オーバーレイ（Picker） ──

    // ⌘P ファイルファインダ。全ファイル列挙は背景（大リポジトリの walk / remote RPC で UI を止めない）。
}

/// トーストに出す名前（パスの末尾）。
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// 取り消しが済んだ時のトースト（何を戻したか）。
fn undo_done_message(step: &[FileOperation]) -> String {
    match step {
        [FileOperation::Created { path }] => {
            i18n::t!("explorer.undid_create", "name" => file_label(path))
        }
        [FileOperation::Moved { from, .. }] => {
            i18n::t!("explorer.undid_move", "name" => file_label(from))
        }
        [FileOperation::Trashed { original, .. }] => {
            i18n::t!("explorer.undid_trash", "name" => file_label(original))
        }
        _ => i18n::t!("explorer.undid_many", "n" => step.len()),
    }
}

/// 取り消せなかった時のトースト（なぜ戻せないか）。
fn undo_error_message(error: &UndoError) -> String {
    match error {
        UndoError::TrashLocationUnknown { original } => {
            i18n::t!("explorer.undo_trash_unknown", "name" => file_label(original))
        }
        UndoError::MissingFromTrash { original } => {
            i18n::t!("explorer.undo_not_in_trash", "name" => file_label(original))
        }
        UndoError::DestinationExists { path } => {
            i18n::t!("explorer.undo_exists", "name" => file_label(path))
        }
        UndoError::SourceMissing { path } => {
            i18n::t!("explorer.undo_missing", "name" => file_label(path))
        }
        UndoError::Io(error) => i18n::t!("explorer.undo_failed", "error" => format!("{error:#}")),
    }
}

/// エクスプローラのファイルを押した時の開き方（[`Workspace::click_explorer_file`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplorerFileClick {
    /// プレビュータブ（O26）。
    Preview,
    /// OS の既定のアプリ（Finder と同じ手癖）。
    DefaultApp,
    /// 普通のタブ（プレビューを外す）。
    Keep,
}

/// 押した回数から開き方を決める。1 回目 = プレビュー・ダブルクリック = 既定のアプリ（この Mac の
/// ファイルだけ）。SSH 先のファイルのダブルクリックと 3 回目以降は普通のタブ（連打でアプリを重ねない）。
fn explorer_file_click(click_count: usize, is_local: bool) -> ExplorerFileClick {
    match click_count {
        0 | 1 => ExplorerFileClick::Preview,
        2 if is_local => ExplorerFileClick::DefaultApp,
        _ => ExplorerFileClick::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_click_opens_the_default_app_only_for_local_files() {
        assert_eq!(explorer_file_click(1, true), ExplorerFileClick::Preview);
        assert_eq!(explorer_file_click(2, true), ExplorerFileClick::DefaultApp);
        assert_eq!(
            explorer_file_click(2, false),
            ExplorerFileClick::Keep,
            "SSH 先のファイルは普通のタブ"
        );
        assert_eq!(
            explorer_file_click(3, true),
            ExplorerFileClick::Keep,
            "連打でアプリを重ねない"
        );
    }

    /// ⌥⌘C: エクスプローラにフォーカスがある時・ファイルを押した直後は、選んでいる項目のパスを
    /// コピーする（Finder と同じ）。エディタに打った後は従来どおり `path:行`。
    #[gpui::test]
    fn copy_path_key_copies_what_you_last_clicked(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_copy_path_key_{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("前回の一時ディレクトリを消す");
        }
        let project = root.join("project");
        std::fs::create_dir_all(project.join("notes")).expect("一時プロジェクト");
        std::fs::write(project.join("a.txt"), "hello\n").expect("a.txt");
        let settings_path = root.join("settings.json");
        std::fs::write(&settings_path, r#"{"onboarded":true}"#).expect("settings");
        cx.update(|cx| {
            settings::init(Some(settings_path), None, cx);
            let bindings = keymap_core::load_bindings(keymap_core::DEFAULT_KEYMAP_JSON, cx)
                .expect("既定 keymap がロードできる");
            cx.bind_keys(bindings);
        });
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            cx.run_until_parked();
        };
        workspace.update_in(cx, |workspace, _window, _cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        draw(cx);
        // Worktree は root を正規化する（/var → /private/var）ので、開いている物から組む。
        let canonical_root = workspace.read_with(cx, |workspace, _cx| {
            workspace
                .active_worktree()
                .map(|worktree| worktree.root().to_path_buf())
                .expect("プロジェクトが開いている")
        });
        let clipboard = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_window, cx| cx.read_from_clipboard().and_then(|item| item.text()))
        };

        // ① フォルダを押した後（フォーカスはエクスプローラ）。
        let folder = canonical_root.join("notes");
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.select_explorer_entry(folder.clone());
            workspace.focus_explorer(window, cx);
        });
        draw(cx);
        cx.simulate_keystrokes("cmd-alt-c");
        assert_eq!(clipboard(cx), Some(folder.display().to_string()));

        // ② ファイルを押した直後（開いたエディタへフォーカスが移っている）も、そのファイルのパス。
        let file = canonical_root.join("a.txt");
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.click_explorer_file(file.clone(), 1, true, window, cx);
        });
        draw(cx);
        cx.simulate_keystrokes("cmd-alt-c");
        assert_eq!(clipboard(cx), Some(file.display().to_string()));

        // ③ エディタに打った後は、従来どおり `path:行`。
        cx.simulate_input("x");
        draw(cx);
        cx.simulate_keystrokes("cmd-alt-c");
        assert_eq!(clipboard(cx).as_deref(), Some("a.txt:1"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 空のプロジェクト（`name` の一時ディレクトリ）を開いた窓。監視は止め、1 回描いてある。
    fn open_empty_project<'a>(
        name: &str,
        cx: &'a mut gpui::TestAppContext,
    ) -> (
        PathBuf,
        PathBuf,
        Entity<Workspace>,
        &'a mut gpui::VisualTestContext,
    ) {
        let root = std::env::temp_dir().join(format!("{name}_{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("前回の一時ディレクトリを消す");
        }
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("空のプロジェクト");
        let settings_path = root.join("settings.json");
        std::fs::write(&settings_path, r#"{"onboarded":true}"#).expect("settings");
        cx.update(|cx| {
            settings::init(Some(settings_path), None, cx);
            let bindings = keymap_core::load_bindings(keymap_core::DEFAULT_KEYMAP_JSON, cx)
                .expect("既定 keymap がロードできる");
            cx.bind_keys(bindings);
        });
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, _window, _cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        draw_explorer(cx);
        // Worktree は root を正規化する（/var → /private/var）ので、開いている物から組む。
        let canonical_root = workspace.read_with(cx, |workspace, _cx| {
            workspace
                .active_worktree()
                .map(|worktree| worktree.root().to_path_buf())
                .expect("プロジェクトが開いている")
        });
        (root, canonical_root, workspace, cx)
    }

    fn draw_explorer(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }

    fn right_click(position: Point<gpui::Pixels>, cx: &mut gpui::VisualTestContext) {
        cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::none());
        cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::none());
        draw_explorer(cx);
    }

    /// 開いている右クリックメニューの (対象, 余白のメニューか)。
    fn open_menu(
        workspace: &Entity<Workspace>,
        cx: &mut gpui::VisualTestContext,
    ) -> Option<(PathBuf, bool)> {
        workspace.read_with(cx, |workspace, cx| {
            workspace
                .explorer_context_menu(cx)
                .map(|menu| (menu.path, menu.background))
        })
    }

    /// メニューの「新規フォルダ」をマウスで押し、`name` を打って ⏎（入力欄にフォーカスがあること）。
    fn create_folder_from_menu(
        workspace: &Entity<Workspace>,
        name: &str,
        cx: &mut gpui::VisualTestContext,
    ) {
        let item = cx
            .debug_bounds("ctx-new-dir")
            .expect("メニューに「新規フォルダ」が出ている");
        cx.simulate_click(item.center(), gpui::Modifiers::none());
        draw_explorer(cx);
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                workspace.explorer_context_menu(cx).is_none(),
                "項目を押したらメニューは閉じる"
            );
            let naming = workspace.explorer_naming(cx).expect("名前の入力が始まる");
            assert_eq!(naming.kind, NamingKind::NewDir);
            assert!(
                naming.focus.is_focused(window),
                "入力欄にフォーカスが無い＝打った名前が入らない"
            );
        });
        cx.simulate_input(name);
        cx.simulate_keystrokes("enter");
        draw_explorer(cx);
    }

    /// 空のフォルダ（行が 1 つも無い）でも、ツリーの余白を右クリックするとルートのメニューが出て、
    /// マウスだけで「新規フォルダ」→ 名前を打って ⏎ まで行ける（2026-10-05 本人要望）。行の右クリックは
    /// 行のメニューのまま（外側の余白のメニューで上書きされない）。
    #[gpui::test]
    fn right_clicking_the_blank_of_an_empty_tree_offers_new_folder(cx: &mut gpui::TestAppContext) {
        let (root, canonical_root, workspace, cx) =
            open_empty_project("necoder_blank_menu_tree", cx);
        let has_rows = workspace.read_with(cx, |workspace, _cx| {
            workspace
                .active_slot()
                .is_some_and(|slot| !slot.explorer.rows.is_empty())
        });
        assert!(!has_rows, "空のフォルダから始める");

        let blank = gpui::point(px(RAIL_WIDTH + 60.), px(TITLEBAR_HEIGHT + 200.));
        right_click(blank, cx);
        assert_eq!(
            open_menu(&workspace, cx),
            Some((canonical_root.clone(), true)),
            "余白の右クリックでルートのメニューが出る"
        );
        let explorer_focused = workspace.update_in(cx, |workspace, window, _cx| {
            workspace.chrome.explorer_focus.is_focused(window)
        });
        assert!(
            explorer_focused,
            "右クリックでフォーカスがエクスプローラへ移る"
        );
        for item in ["ctx-new-file", "ctx-reveal", "ctx-copy"] {
            assert!(
                cx.debug_bounds(item).is_some(),
                "余白のメニューに {item} が無い"
            );
        }
        for item in ["ctx-rename", "ctx-duplicate", "ctx-trash", "ctx-open-here"] {
            assert!(
                cx.debug_bounds(item).is_none(),
                "余白のメニューに項目そのものへの操作 {item} が出ている"
            );
        }

        create_folder_from_menu(&workspace, "assets", cx);
        let assets = canonical_root.join("assets");
        assert!(assets.is_dir(), "打った名前でフォルダができる");
        let rows = workspace.read_with(cx, |workspace, _cx| {
            workspace
                .active_slot()
                .map(|slot| {
                    slot.explorer
                        .rows
                        .iter()
                        .map(|row| row.path.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        assert_eq!(rows, vec![assets.clone()], "作ったフォルダがツリーに出る");

        // 余白は下の方を押すことが多い。窓の下端の近くでも、メニューは窓の中へ押し戻される。
        let viewport = cx.update(|window, _cx| window.viewport_size());
        right_click(
            gpui::point(px(RAIL_WIDTH + 60.), viewport.height - px(80.)),
            cx,
        );
        assert_eq!(
            open_menu(&workspace, cx),
            Some((canonical_root.clone(), true))
        );
        let last_item = cx
            .debug_bounds("ctx-copy")
            .expect("余白のメニューに「パスをコピー」");
        assert!(
            last_item.bottom() <= viewport.height,
            "メニューが窓の下で切れている: {last_item:?} / {viewport:?}"
        );

        // 行の右クリックは行のメニュー（余白のメニューに上書きされない）。
        let first_row = gpui::point(
            px(RAIL_WIDTH + 60.),
            px(TITLEBAR_HEIGHT + 28. + ROW_HEIGHT / 2. + 2.),
        );
        right_click(first_row, cx);
        assert_eq!(open_menu(&workspace, cx), Some((assets.clone(), false)));

        // 行のメニューの「名前を変更…」もマウスで押して打てる（メニューの押下が下へ通らない）。
        let rename = cx
            .debug_bounds("ctx-rename")
            .expect("行のメニューに「名前を変更…」");
        cx.simulate_click(rename.center(), gpui::Modifiers::none());
        draw_explorer(cx);
        let rename_focused = workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .explorer_naming(cx)
                .is_some_and(|naming| naming.focus.is_focused(window))
        });
        assert!(rename_focused, "名前の変更の入力欄にフォーカスが無い");
        cx.simulate_input("2");
        cx.simulate_keystrokes("enter");
        draw_explorer(cx);
        assert!(
            canonical_root.join("assets2").is_dir() && !assets.exists(),
            "打った名前に変わる"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// アイコン表示・カラム表示でも余白の右クリックが効き、アイコン表示では名前の入力がセルに出て
    /// 打てる（入力はツリーにしか無かった）。カラムの余白は、その段のフォルダが対象。
    #[gpui::test]
    fn icons_and_columns_offer_the_blank_menu_too(cx: &mut gpui::TestAppContext) {
        let (root, canonical_root, workspace, cx) =
            open_empty_project("necoder_blank_menu_views", cx);
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.set_explorer_view(ExplorerView::Icons, cx)
        });
        draw_explorer(cx);
        let blank = gpui::point(px(RAIL_WIDTH + 60.), px(TITLEBAR_HEIGHT + 200.));
        right_click(blank, cx);
        assert_eq!(
            open_menu(&workspace, cx),
            Some((canonical_root.clone(), true)),
            "アイコン表示の余白 = 現在フォルダのメニュー"
        );
        create_folder_from_menu(&workspace, "docs", cx);
        let docs = canonical_root.join("docs");
        assert!(
            docs.is_dir(),
            "アイコン表示でも打った名前でフォルダができる"
        );

        // カラム表示で docs に入る: [ルート | docs] の 2 段。docs の段の余白 = docs、段より右も docs。
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.set_explorer_view(ExplorerView::Columns, cx);
            workspace.enter_dir(docs.clone(), cx);
        });
        draw_explorer(cx);
        right_click(
            gpui::point(px(RAIL_WIDTH + 150. + 60.), px(TITLEBAR_HEIGHT + 200.)),
            cx,
        );
        assert_eq!(open_menu(&workspace, cx), Some((docs.clone(), true)));
        right_click(
            gpui::point(px(RAIL_WIDTH + 60.), px(TITLEBAR_HEIGHT + 200.)),
            cx,
        );
        assert_eq!(
            open_menu(&workspace, cx),
            Some((canonical_root.clone(), true)),
            "手前の段の余白はその段のフォルダ"
        );
        right_click(
            gpui::point(px(RAIL_WIDTH + 300. + 60.), px(TITLEBAR_HEIGHT + 200.)),
            cx,
        );
        assert_eq!(
            open_menu(&workspace, cx),
            Some((docs, true)),
            "段より右は現在フォルダ"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
