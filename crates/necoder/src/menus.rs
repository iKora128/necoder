//! menus — macOS ネイティブメニューバー（M13 公開準備）。
//!
//! アクションは keymap / コマンドパレットと**同じ dispatch 系**に流れる（gpui の
//! `on_app_menu_action` → `dispatch_action` = アクティブウィンドウのフォーカス文脈で解決）。
//! キー表記（⌘S 等）は gpui が keymap から自動で付ける = ここでは書かない。
//! ラベルは i18n（`menu.*`・ja/en 両方）。Zed の `zed/src/zed/app_menus.rs` と同じ置き場
//! （メニュー = アプリの組み立て = bin crate の責務）。
//!
//! 注意: `cx.set_menus` は呼び出し時点の keymap のスナップショットでキー表記を解決する。
//! ユーザー keymap の live reload 後は main 側で `set_menus` を呼び直す。

use gpui::{Menu, MenuItem, OsAction, SystemMenuType};
use i18n::t;

/// メニューバー全体。macOS 以外では gpui 側が無視する（設定しても無害）。
pub fn app_menus() -> Vec<Menu> {
    vec![
        // ── アプリメニュー（名前は表示されず、OS がアプリ名を出す） ──
        Menu::new("necoder").items(vec![
            MenuItem::action(t!("menu.about"), workspace::About),
            MenuItem::action(t!("menu.check_updates"), workspace::CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action(t!("menu.settings"), workspace::OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(t!("menu.services"), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t!("menu.hide"), workspace::Hide),
            MenuItem::action(t!("menu.hide_others"), workspace::HideOthers),
            MenuItem::action(t!("menu.show_all"), workspace::ShowAll),
            MenuItem::separator(),
            MenuItem::action(t!("menu.quit"), crate::Quit),
        ]),
        // VSCode 並び: New Window → 開く…/Open Recent → ファイル/プロジェクト → 保存 → 閉じる。
        Menu::new(t!("menu.file")).items(vec![
            MenuItem::action(t!("menu.new_window"), workspace::NewWindow),
            MenuItem::separator(),
            MenuItem::action(t!("menu.open_dialog"), workspace::OpenDialog),
            MenuItem::action(t!("menu.open_recent"), workspace::OpenRecent),
            MenuItem::separator(),
            MenuItem::action(t!("menu.open_file"), workspace::FileFinder),
            MenuItem::action(t!("menu.open_project"), workspace::ProjectSwitcher),
            MenuItem::action(t!("menu.open_remote"), workspace::RemoteSsh),
            MenuItem::separator(),
            MenuItem::action(t!("menu.save"), workspace::SaveActive),
            MenuItem::separator(),
            MenuItem::action(t!("menu.close_tab"), workspace::CloseTab),
            MenuItem::action(t!("menu.restore_tab"), workspace::RestoreClosedTab),
        ]),
        Menu::new(t!("menu.edit")).items(vec![
            MenuItem::os_action(t!("menu.undo"), editor_view::Undo, OsAction::Undo),
            MenuItem::os_action(t!("menu.redo"), editor_view::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t!("menu.cut"), editor_view::Cut, OsAction::Cut),
            MenuItem::os_action(t!("menu.copy"), editor_view::Copy, OsAction::Copy),
            MenuItem::os_action(t!("menu.paste"), editor_view::Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action(
                t!("menu.select_all"),
                editor_view::SelectAll,
                OsAction::SelectAll,
            ),
            MenuItem::action(t!("menu.select_next"), editor_view::SelectNext),
            MenuItem::action(t!("menu.add_cursor_above"), editor_view::AddCursorAbove),
            MenuItem::action(t!("menu.add_cursor_below"), editor_view::AddCursorBelow),
            MenuItem::separator(),
            MenuItem::action(t!("menu.move_line_up"), editor_view::MoveLineUp),
            MenuItem::action(t!("menu.move_line_down"), editor_view::MoveLineDown),
            MenuItem::action(t!("menu.duplicate_line"), editor_view::DuplicateLineDown),
            MenuItem::action(t!("menu.delete_line"), editor_view::DeleteLine),
            MenuItem::action(t!("menu.toggle_comment"), editor_view::ToggleComment),
            MenuItem::separator(),
            MenuItem::action(t!("menu.find"), workspace::BufferSearch),
            MenuItem::action(t!("menu.replace"), workspace::BufferReplace),
            MenuItem::action(t!("menu.find_project"), workspace::ProjectSearch),
            MenuItem::action(t!("menu.references"), workspace::FindReferences),
            MenuItem::separator(),
            MenuItem::action(t!("menu.format"), workspace::Format),
            MenuItem::action(t!("menu.rename_symbol"), workspace::Rename),
            MenuItem::action(t!("menu.code_actions"), workspace::CodeActions),
            MenuItem::action(t!("menu.inline_edit"), workspace::InlineEdit),
        ]),
        Menu::new(t!("menu.view")).items(vec![
            MenuItem::action(t!("menu.command_palette"), workspace::CommandPalette),
            MenuItem::separator(),
            MenuItem::action(t!("menu.terminal"), workspace::ToggleTerminal),
            MenuItem::action(t!("menu.git_panel"), workspace::ToggleGitPanel),
            MenuItem::action(t!("menu.todo_board"), workspace::ToggleTodoBoard),
            MenuItem::action(t!("menu.herd"), workspace::ToggleHerdSidebar),
            MenuItem::action(t!("menu.fleet"), workspace::ToggleFleet),
            MenuItem::action(
                t!("menu.agent_full_screen"),
                workspace::ToggleAgentFullScreen,
            ),
            MenuItem::action(t!("menu.diagnostics"), workspace::DiagnosticsPanel),
            MenuItem::separator(),
            MenuItem::action(t!("menu.split_right"), workspace::SplitRight),
            MenuItem::action(t!("menu.soft_wrap"), editor_view::ToggleSoftWrap),
            MenuItem::separator(),
            MenuItem::action(t!("menu.theme"), workspace::ThemeSelector),
            MenuItem::action(t!("menu.project_color"), workspace::ProjectColor),
        ]),
        Menu::new(t!("menu.go")).items(vec![
            MenuItem::action(t!("menu.back"), workspace::NavigateBack),
            MenuItem::action(t!("menu.forward"), workspace::NavigateForward),
            MenuItem::separator(),
            MenuItem::action(t!("menu.goto_line"), workspace::GoToLine),
            MenuItem::action(t!("menu.outline"), workspace::OutlineSymbols),
            MenuItem::action(t!("menu.workspace_symbols"), workspace::WorkspaceSymbols),
            MenuItem::action(t!("menu.definition"), workspace::GoToDefinition),
            MenuItem::separator(),
            MenuItem::action(t!("menu.next_diagnostic"), workspace::NextDiagnostic),
            MenuItem::action(t!("menu.prev_diagnostic"), workspace::PrevDiagnostic),
            MenuItem::action(t!("menu.next_hunk"), workspace::NextHunk),
            MenuItem::action(t!("menu.prev_hunk"), workspace::PrevHunk),
            MenuItem::separator(),
            MenuItem::action(t!("menu.next_tab"), workspace::SelectNextTab),
            MenuItem::action(t!("menu.prev_tab"), workspace::SelectPrevTab),
        ]),
        Menu::new(t!("menu.ai")).items(vec![
            MenuItem::action(t!("menu.new_thread"), workspace::NewThread),
            MenuItem::action(t!("menu.thread_history"), workspace::ThreadHistory),
            MenuItem::separator(),
            MenuItem::action(t!("menu.next_thread"), workspace::SelectNextThread),
            MenuItem::action(t!("menu.prev_thread"), workspace::SelectPrevThread),
        ]),
        Menu::new(t!("menu.window")).items(vec![
            MenuItem::action(t!("menu.minimize"), workspace::Minimize),
            MenuItem::action(t!("menu.zoom"), workspace::Zoom),
        ]),
        Menu::new(t!("menu.help")).items(vec![MenuItem::action(
            t!("menu.report_bug"),
            workspace::ReportBug,
        )]),
    ]
}

/// Dock アイコン右クリックのメニュー。
pub fn dock_menu() -> Vec<MenuItem> {
    vec![MenuItem::action(
        t!("menu.new_window"),
        workspace::NewWindow,
    )]
}
