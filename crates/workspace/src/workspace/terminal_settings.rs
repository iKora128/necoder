//! ターミナルの設定（O25）を settings.json から terminal_view へ渡す。
//!
//! terminal_view は設定の crate を知らない（依存の向き）。ここで設定を
//! [`terminal_view::TerminalAppearance`]（見た目）と [`terminal_view::TerminalShell`]（手元で開く
//! シェル）に写して global に置き、設定が変わったら置き直す。開いている端末は見た目の global を
//! 見張っていて、その場で描き直す。シェルは新しく開く端末から効く。

use crate::workspace::*;

/// 設定からターミナルの見た目を作る（範囲の外の値は terminal_view が丸める）。フォントが空なら
/// コードの書体（`code_font_family`・それも空なら同梱の既定）に揃える。
pub(crate) fn terminal_appearance_from(
    settings: &settings::Settings,
) -> terminal_view::TerminalAppearance {
    let family = if settings.terminal_font_family.trim().is_empty() {
        &settings.code_font_family
    } else {
        &settings.terminal_font_family
    };
    terminal_view::TerminalAppearance::new(
        settings.terminal_font_size,
        family,
        usize::try_from(settings.terminal_scrollback).unwrap_or(usize::MAX),
        &settings.terminal_cursor,
    )
    .with_colors(super::terminal_colors::load_color_scheme(
        &settings.terminal_color_scheme,
    ))
}

/// 設定から手元で開くシェルを作る（空 = OS の既定）。
pub(crate) fn terminal_shell_from(settings: &settings::Settings) -> terminal_view::TerminalShell {
    terminal_view::TerminalShell {
        program: settings.terminal_shell.trim().to_string(),
        args: settings.terminal_shell_args.clone(),
    }
}

/// 設定からよく使うコマンドを作る（名前かコマンドが空の物は飛ばす）。
pub(crate) fn quick_commands_from(settings: &settings::Settings) -> terminal_view::QuickCommands {
    terminal_view::QuickCommands(
        settings
            .quick_commands
            .iter()
            .filter(|quick| !quick.name.trim().is_empty() && !quick.command.trim().is_empty())
            .map(|quick| terminal_view::QuickCommand {
                name: quick.name.trim().to_string(),
                command: quick.command.trim().to_string(),
            })
            .collect(),
    )
}

/// 起動時に 1 回だけ繋ぐ（main から呼ぶ）: 今の設定を置き、設定が変わるたびに置き直す。
pub fn install_terminal_settings(cx: &mut App) {
    refresh_terminal_settings(cx);
    cx.observe_global::<settings::SettingsGlobal>(refresh_terminal_settings)
        .detach();
}

/// 設定から作り直す。前と同じなら置かない（ほかの設定の変更で端末を起こさない）。
fn refresh_terminal_settings(cx: &mut App) {
    let Some((appearance, shell, quick)) =
        cx.try_global::<settings::SettingsGlobal>().map(|global| {
            (
                terminal_appearance_from(global.settings()),
                terminal_shell_from(global.settings()),
                quick_commands_from(global.settings()),
            )
        })
    else {
        return;
    };
    if cx.try_global::<terminal_view::TerminalAppearance>() != Some(&appearance) {
        cx.set_global(appearance);
    }
    if cx.try_global::<terminal_view::TerminalShell>() != Some(&shell) {
        cx.set_global(shell);
    }
    if cx.try_global::<terminal_view::QuickCommands>() != Some(&quick) {
        cx.set_global(quick);
        // ドックの ▶ は描画のたびに読むので、開いている窓を描き直させる。
        cx.refresh_windows();
    }
}

/// `/etc/shells` の書き方（`#` はコメント・1 行 1 パス）から、実在するシェルを並べる（重複は外す）。
/// `extra`（`$SHELL`）は一覧に無ければ先頭に足す。
pub(crate) fn shells_from_list(
    text: &str,
    extra: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> Vec<String> {
    let mut shells: Vec<String> = Vec::new();
    let candidates = extra
        .into_iter()
        .chain(text.lines().map(str::trim))
        .filter(|line| line.starts_with('/'));
    for shell in candidates {
        if !shells.iter().any(|known| known == shell) && exists(Path::new(shell)) {
            shells.push(shell.to_string());
        }
    }
    shells
}

/// 手元で開けるシェルの一覧（O25・シェルを選ぶ画面）。mac / Linux は `/etc/shells` と `$SHELL`、
/// Windows は PATH にある pwsh / powershell / cmd / bash（Git Bash）/ wsl。
pub(crate) fn available_shells() -> Vec<String> {
    if cfg!(windows) {
        let paths: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        ["pwsh", "powershell", "cmd", "bash", "wsl"]
            .into_iter()
            .filter(|name| {
                paths
                    .iter()
                    .any(|directory| directory.join(format!("{name}.exe")).is_file())
            })
            .map(str::to_string)
            .collect()
    } else {
        let listed = std::fs::read_to_string("/etc/shells").unwrap_or_default();
        let current = std::env::var("SHELL").ok();
        shells_from_list(&listed, current.as_deref(), |path| path.is_file())
    }
}

impl Workspace {
    /// シェルのピッカーを開く（O25）。先頭 = 既定に戻す、続けて入っているシェル。
    pub(crate) fn open_shell_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shells = available_shells();
        let current = settings::get(cx).terminal_shell.trim().to_string();
        let mut items = vec![PickerItem::new(
            0,
            i18n::t!("settings.shell_picker_default"),
        )];
        items.extend(shells.iter().enumerate().map(|(index, shell)| {
            let name = Path::new(shell)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| shell.clone());
            let item = PickerItem::new(index + 1, name).with_detail(shell.clone());
            if *shell == current {
                item.with_accent(self.accent())
            } else {
                item
            }
        }));
        self.overlays.picker_shells = shells;
        self.open_picker(
            PickerMode::Shells,
            i18n::t!("settings.shell_picker_placeholder"),
            items,
            window,
            cx,
        );
    }

    /// 選んだシェルを `terminal_shell` へ書く（id 0 = 空 = 既定）。引数（`terminal_shell_args`）は触らない。
    pub(crate) fn commit_shell(&mut self, id: usize, cx: &mut Context<Self>) {
        let value = match id {
            0 => String::new(),
            index => match self.overlays.picker_shells.get(index - 1) {
                Some(shell) => shell.clone(),
                None => return,
            },
        };
        let result =
            settings::set_user_value(cx, "terminal_shell", serde_json::Value::String(value));
        self.report_settings_save(result, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O25: よく使うコマンドは設定から並び（名前かコマンドが空の物は飛ばす）、選ぶと新しい端末を
    /// 開いてそこへ打つ（Enter まで）。
    #[gpui::test]
    fn a_quick_command_runs_in_a_new_terminal(cx: &mut gpui::TestAppContext) {
        let settings = settings::Settings {
            quick_commands: vec![
                settings::QuickCommandSetting {
                    name: "開発サーバ".to_string(),
                    command: " npm run dev ".to_string(),
                },
                settings::QuickCommandSetting {
                    name: " ".to_string(),
                    command: "ignored".to_string(),
                },
            ],
            ..settings::Settings::default()
        };
        let quick = quick_commands_from(&settings);
        assert_eq!(
            quick.0,
            vec![terminal_view::QuickCommand {
                name: "開発サーバ".to_string(),
                command: "npm run dev".to_string(),
            }]
        );
        cx.update(|cx| cx.set_global(quick));
        let (dock, cx) = cx.add_window_view(|_, _cx| {
            let mut dock = terminal_view::TerminalDock::new(
                terminal_view::TerminalLaunch::default(),
                Theme::dark(),
            );
            dock.use_test_terminals();
            dock
        });
        dock.update_in(cx, |dock, window, cx| {
            let before = dock.ensure_active(cx);
            dock.run_quick_command(0, window, cx);
            let terminal = dock.ensure_active(cx);
            assert_ne!(terminal, before, "新しい端末で走らせる");
            assert_eq!(terminal.read(cx).debug_written_input(), b"npm run dev\r");
            assert!(
                before.read(cx).debug_written_input().is_empty(),
                "今の端末には打たない"
            );
        });
    }

    #[test]
    fn settings_become_the_terminal_appearance() {
        let settings = settings::Settings {
            terminal_font_size: 15.0,
            terminal_font_family: "Menlo".to_string(),
            terminal_scrollback: 2_000,
            terminal_cursor: "underline".to_string(),
            terminal_shell: " fish ".to_string(),
            terminal_shell_args: vec!["-l".to_string()],
            ..settings::Settings::default()
        };
        let appearance = terminal_appearance_from(&settings);
        assert_eq!(appearance.font_size, 15.0);
        assert_eq!(appearance.font_family.as_ref(), "Menlo");
        assert_eq!(appearance.scrollback, 2_000);
        assert_eq!(appearance.cursor, terminal_view::TerminalCursor::Underline);
        assert_eq!(
            terminal_shell_from(&settings),
            terminal_view::TerminalShell {
                program: "fish".to_string(),
                args: vec!["-l".to_string()],
            }
        );
        assert_eq!(
            terminal_appearance_from(&settings::Settings::default()),
            terminal_view::TerminalAppearance::default(),
            "既定の設定は設定を持つ前の見た目"
        );
    }

    #[test]
    fn shells_come_from_the_list_and_the_login_shell() {
        let list =
            "# /etc/shells\n/bin/bash\n/bin/zsh\n/usr/local/bin/fish\n/bin/zsh\nnot-a-path\n";
        let exists = |path: &Path| path != Path::new("/usr/local/bin/fish");
        assert_eq!(
            shells_from_list(list, Some("/opt/homebrew/bin/nu"), exists),
            vec![
                "/opt/homebrew/bin/nu".to_string(),
                "/bin/bash".to_string(),
                "/bin/zsh".to_string(),
            ],
            "ログインシェルが先頭・無いものと重複は外す"
        );
        assert_eq!(
            shells_from_list(list, Some("/bin/zsh"), |_| true).len(),
            3,
            "一覧にあるログインシェルは重ねない"
        );
    }

    #[gpui::test]
    fn the_shell_picker_writes_the_chosen_shell(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_shell_picker_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.chrome.pending_shell_picker = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        workspace.update_in(cx, |workspace, _window, cx| {
            assert!(workspace.overlays.picker_mode == PickerMode::Shells);
            workspace.overlays.picker_shells = vec!["/bin/zsh".to_string()];
            workspace.commit_shell(1, cx);
            assert_eq!(settings::get(cx).terminal_shell, "/bin/zsh");
            assert_eq!(
                terminal_shell_from(&settings::get(cx)).program,
                "/bin/zsh",
                "新しく開く端末のシェルになる"
            );
            workspace.commit_shell(0, cx);
            assert_eq!(settings::get(cx).terminal_shell, "");
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
