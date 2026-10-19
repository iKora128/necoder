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

/// シェルのピッカーの 1 行（O25）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellChoice {
    pub(crate) kind: ShellKind,
    /// `terminal_shell` に書く物。
    pub(crate) program: String,
    /// `Some` なら選んだ時に `terminal_shell_args` も書く（Windows の Git Bash・WSL はシェルと引数の組で
    /// 1 つ）。mac / Linux は `None` ＝ 引数には触らない（従来どおり・WINDOWS-PORT.md §D8）。
    pub(crate) args: Option<Vec<String>>,
}

/// シェルの種類（表示名を決める）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShellKind {
    /// mac / Linux の `/etc/shells` の 1 行（名前はファイル名）。
    Path,
    /// PowerShell 7（`pwsh`）。
    Pwsh,
    /// OS 同梱の Windows PowerShell 5.1。
    WindowsPowerShell,
    /// コマンド プロンプト（`cmd`）。
    Cmd,
    /// Git for Windows の bash。
    GitBash,
    /// WSL のディストリビューション 1 つ。
    Wsl { distro: String },
}

impl ShellChoice {
    fn label(&self) -> String {
        match &self.kind {
            ShellKind::Path => Path::new(&self.program)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| self.program.clone()),
            ShellKind::Pwsh => i18n::t!("settings.shell_pwsh"),
            ShellKind::WindowsPowerShell => i18n::t!("settings.shell_windows_powershell"),
            ShellKind::Cmd => i18n::t!("settings.shell_cmd"),
            ShellKind::GitBash => i18n::t!("settings.shell_git_bash"),
            ShellKind::Wsl { distro } => i18n::t!("settings.shell_wsl", "distro" => distro),
        }
    }

    /// ピッカーの右に出す、実際に起こすコマンド。
    fn detail(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().flatten().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// 今の設定がこの行か（引数を持つ行は引数まで同じ時だけ）。
    fn is_current(&self, program: &str, args: &[String]) -> bool {
        self.program == program
            && self
                .args
                .as_ref()
                .is_none_or(|own| own.as_slice() == args)
    }
}

/// 「いつものシェル」の行（設定のシェル。設定が空なら OS の既定 = Windows は PowerShell 7 →
/// Windows PowerShell の先に在る方・mac / Linux は `$SHELL`）。
pub(crate) fn usual_shell(shells: &[ShellChoice], current: &str, args: &[String]) -> Option<usize> {
    if !current.is_empty() {
        return shells.iter().position(|shell| shell.is_current(current, args));
    }
    if cfg!(windows) {
        return shells.iter().position(|shell| {
            matches!(shell.kind, ShellKind::Pwsh | ShellKind::WindowsPowerShell)
        });
    }
    let login = std::env::var("SHELL").ok()?;
    shells.iter().position(|shell| shell.program == login)
}

/// 手元で開けるシェルの一覧（O25・シェルを選ぶ画面）。mac / Linux は `/etc/shells` と `$SHELL`、
/// Windows は PowerShell 7 / Windows PowerShell / コマンド プロンプト / Git Bash / WSL の各ディストリ。
/// **blocking**（Windows は `wsl.exe` を 1 回起こす）なので背景で呼ぶ。
pub(crate) fn available_shells() -> Vec<ShellChoice> {
    if cfg!(windows) {
        let program_dirs: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .chain(std::env::var_os("LOCALAPPDATA").map(|local| PathBuf::from(local).join("Programs")))
            .collect();
        let scoop = paths::home_dir().map(|home| home.join(r"scoop\apps\git\current"));
        let git_bash = git_bash_candidates(host::find_in_path("git").as_deref(), &program_dirs, scoop);
        let has_wsl = host::find_in_path("wsl").is_some();
        let distros = if has_wsl { wsl_distros() } else { Vec::new() };
        windows_shells(host::find_in_path, &git_bash, |path| path.is_file(), &distros)
    } else {
        let listed = std::fs::read_to_string("/etc/shells").unwrap_or_default();
        let current = std::env::var("SHELL").ok();
        shells_from_list(&listed, current.as_deref(), |path| path.is_file())
            .into_iter()
            .map(|program| ShellChoice {
                kind: ShellKind::Path,
                program,
                args: None,
            })
            .collect()
    }
}

/// Windows のシェルを並べる（探し方は引数で受け取る＝どのプラットフォームでもテストできる）。
///
/// - PowerShell 7 → Windows PowerShell → コマンド プロンプト（PATH にある物）
/// - Git Bash: `git_bash` の候補で最初に在る物。**PATH の `bash` は見ない** — Windows の
///   `System32\bash.exe` は WSL の入口で、Git Bash は既定では PATH に入らない（`Git\cmd` だけが入る）
/// - WSL: ディストリごとに 1 行（`wsl -d <名前>`）。端末の作業フォルダ（プロジェクトの根）は
///   `wsl.exe` が `/mnt/c/...` に読み替えて引き継ぐ
pub(crate) fn windows_shells(
    find: impl Fn(&str) -> Option<PathBuf>,
    git_bash: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
    wsl_distros: &[String],
) -> Vec<ShellChoice> {
    let plain = |kind: ShellKind, program: &str| ShellChoice {
        kind,
        program: program.to_string(),
        args: Some(Vec::new()),
    };
    let mut shells = Vec::new();
    for (kind, program) in [
        (ShellKind::Pwsh, "pwsh"),
        (ShellKind::WindowsPowerShell, "powershell"),
        (ShellKind::Cmd, "cmd"),
    ] {
        if find(program).is_some() {
            shells.push(plain(kind, program));
        }
    }
    if let Some(bash) = git_bash.iter().find(|candidate| exists(candidate)) {
        shells.push(ShellChoice {
            kind: ShellKind::GitBash,
            program: bash.to_string_lossy().into_owned(),
            // ログインシェルとして起こす（/etc/profile が PATH と見た目を整える・Git Bash の窓と同じ）。
            args: Some(vec!["--login".to_string(), "-i".to_string()]),
        });
    }
    shells.extend(wsl_distros.iter().map(|distro| ShellChoice {
        kind: ShellKind::Wsl {
            distro: distro.clone(),
        },
        program: "wsl".to_string(),
        args: Some(vec!["-d".to_string(), distro.clone()]),
    }));
    shells
}

/// Git Bash（`<Git>\bin\bash.exe`）の候補を、確からしい順に並べる（重複は外す）。
/// 1. PATH の `git.exe` が居る Git（`<Git>\cmd\git.exe` / `<Git>\bin\git.exe` / `<Git>\mingw64\bin\git.exe`）
/// 2. 既定の置き場（Program Files・ユーザー単位の `%LOCALAPPDATA%\Programs`）
/// 3. Scoop
pub(crate) fn git_bash_candidates(
    git: Option<&Path>,
    program_dirs: &[PathBuf],
    scoop: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(directory) = git.and_then(Path::parent) {
        let name = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
        };
        let root = match name(directory).as_deref() {
            Some("cmd") => directory.parent(),
            Some("bin") => directory
                .parent()
                .filter(|parent| name(parent).as_deref() != Some("mingw64"))
                .or_else(|| directory.parent().and_then(Path::parent)),
            _ => None,
        };
        roots.extend(root.map(Path::to_path_buf));
    }
    roots.extend(program_dirs.iter().map(|directory| directory.join("Git")));
    roots.extend(scoop);
    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in roots {
        let bash = root.join("bin").join("bash.exe");
        let key = bash.to_string_lossy().to_lowercase();
        if !candidates
            .iter()
            .any(|known| known.to_string_lossy().to_lowercase() == key)
        {
            candidates.push(bash);
        }
    }
    candidates
}

/// `wsl.exe -l -q` の出力をディストリの名前にする。Docker Desktop が置く内部用（`docker-desktop` /
/// `docker-desktop-data`）は端末に向かないので外す。
///
/// 出力は**BOM の無い UTF-16LE**（`WSL_UTF8=1` を解する版なら UTF-8）。どちらでも読めるよう、
/// NUL を含めば UTF-16LE として読む。
pub(crate) fn parse_wsl_distros(output: &[u8]) -> Vec<String> {
    let text = if output.contains(&0) {
        let units: Vec<u16> = output
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(output).into_owned()
    };
    text.lines()
        .map(|line| line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}'))
        .filter(|name| !name.is_empty() && !name.to_lowercase().starts_with("docker-desktop"))
        .map(str::to_string)
        .collect()
}

/// 入っている WSL のディストリ（無い・WSL 自体が無い・応答しない時は空）。
/// WSL の仮想マシンが止まっていると起こすのに数秒かかることがあるので、待つのは 5 秒まで。
fn wsl_distros() -> Vec<String> {
    let child = child_process::command("wsl")
        .args(["-l", "-q"])
        .env("WSL_UTF8", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            eprintln!("wsl の一覧を読めない: {error}");
            return Vec::new();
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // ディストリが 1 つも無い時は失敗の終了コードで案内文を出す＝空として扱う。
                if !status.success() {
                    return Vec::new();
                }
                let mut output = Vec::new();
                if let Some(mut stdout) = child.stdout.take() {
                    if let Err(error) = std::io::Read::read_to_end(&mut stdout, &mut output) {
                        eprintln!("wsl の一覧を読めない: {error}");
                    }
                }
                return parse_wsl_distros(&output);
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                eprintln!("wsl -l -q が 5 秒で返らないため WSL のシェルを一覧に出さない");
                if let Err(error) = child.kill() {
                    eprintln!("wsl を止められない: {error}");
                }
                return Vec::new();
            }
            Err(error) => {
                eprintln!("wsl の終わりを待てない: {error}");
                return Vec::new();
            }
        }
    }
}

impl Workspace {
    /// シェルのピッカーを開く（O25）。先頭 = 既定に戻す、続けて入っているシェル。
    /// 一覧は背景で集める（Windows は WSL のディストリを `wsl.exe` に聞くため）。
    pub(crate) fn open_shell_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shells = cx.background_executor().spawn(async { available_shells() });
        cx.spawn_in(window, async move |workspace, cx| {
            let shells = shells.await;
            let shown = workspace.update_in(cx, |workspace, window, cx| {
                workspace.show_shell_picker(shells, window, cx)
            });
            if let Err(error) = shown {
                eprintln!("シェルのピッカーを開けない（窓が既に無い）: {error:#}");
            }
        })
        .detach();
    }

    fn show_shell_picker(
        &mut self,
        shells: Vec<ShellChoice>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = settings::get(cx);
        let current = settings.terminal_shell.trim().to_string();
        let current_args = settings.terminal_shell_args.clone();
        let mut items = vec![PickerItem::new(
            0,
            i18n::t!("settings.shell_picker_default"),
        )];
        items.extend(shells.iter().enumerate().map(|(index, shell)| {
            let item = PickerItem::new(index + 1, shell.label()).with_detail(shell.detail());
            if shell.is_current(&current, &current_args) {
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

    /// ターミナルの「▾」: シェルを選んで新しい端末を開く（設定は変えない）。いつものシェル（設定・
    /// 空なら OS の既定）の行に「（いつものシェル）」を添える。一覧は背景で集める（WSL に聞くため）。
    pub(crate) fn open_new_terminal_shell_picker(
        &mut self,
        dock: Entity<terminal_view::TerminalDock>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shells = cx.background_executor().spawn(async { available_shells() });
        let dock = dock.downgrade();
        cx.spawn_in(window, async move |workspace, cx| {
            let shells = shells.await;
            let shown = workspace.update_in(cx, |workspace, window, cx| {
                let settings = settings::get(cx);
                let current = settings.terminal_shell.trim().to_string();
                let current_args = settings.terminal_shell_args.clone();
                let usual = usual_shell(&shells, &current, &current_args);
                let items = shells
                    .iter()
                    .enumerate()
                    .map(|(index, shell)| {
                        let label = if Some(index) == usual {
                            format!("{} {}", shell.label(), i18n::t!("terminal.shell_menu_default"))
                        } else {
                            shell.label()
                        };
                        PickerItem::new(index, label).with_detail(shell.detail())
                    })
                    .collect();
                workspace.overlays.picker_shells = shells;
                workspace.overlays.picker_shell_dock = Some(dock);
                workspace.open_picker(
                    PickerMode::ShellsForNewTerminal,
                    i18n::t!("terminal.shell_menu_placeholder"),
                    items,
                    window,
                    cx,
                );
            });
            if let Err(error) = shown {
                eprintln!("シェルのメニューを開けない（窓が既に無い）: {error:#}");
            }
        })
        .detach();
    }

    /// 「▾」で選んだシェルで、頼んだドックに新しい端末を開く（タブ名はシェルの名前）。
    pub(crate) fn commit_new_terminal_shell(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shell) = self.overlays.picker_shells.get(id).cloned() else {
            return;
        };
        let Some(dock) = self
            .overlays
            .picker_shell_dock
            .take()
            .and_then(|dock| dock.upgrade())
        else {
            return;
        };
        let title = Some(shell.label());
        dock.update(cx, |dock, cx| {
            dock.add_with_shell(
                shell.program.clone(),
                shell.args.clone().unwrap_or_default(),
                title,
                window,
                cx,
            )
        });
        if self.terminal_dock == dock {
            self.chrome.show_bottom = true;
        }
        cx.notify();
    }

    /// 選んだシェルを `terminal_shell` へ書く（id 0 = 空 = 既定）。引数（`terminal_shell_args`）は
    /// その行が引数を持つ時だけ書く（Windows の Git Bash・WSL。mac / Linux は触らない）。
    pub(crate) fn commit_shell(&mut self, id: usize, cx: &mut Context<Self>) {
        let (value, args) = match id {
            0 => (String::new(), None),
            index => match self.overlays.picker_shells.get(index - 1) {
                Some(shell) => (shell.program.clone(), shell.args.clone()),
                None => return,
            },
        };
        if let Some(args) = args {
            let args = serde_json::Value::Array(args.into_iter().map(Into::into).collect());
            let result = settings::set_user_value(cx, "terminal_shell_args", args);
            self.report_settings_save(result, cx);
        }
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
        // 一覧は背景で集めてから開く（Windows は wsl.exe に聞くため）。
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            assert!(workspace.overlays.picker_mode == PickerMode::Shells);
            workspace.overlays.picker_shells = vec![
                ShellChoice {
                    kind: ShellKind::Path,
                    program: "/bin/zsh".to_string(),
                    args: None,
                },
                ShellChoice {
                    kind: ShellKind::Wsl {
                        distro: "Ubuntu".to_string(),
                    },
                    program: "wsl".to_string(),
                    args: Some(vec!["-d".to_string(), "Ubuntu".to_string()]),
                },
            ];
            settings::set_user_value(
                cx,
                "terminal_shell_args",
                serde_json::json!(["-l"]),
            )
            .unwrap();
            workspace.commit_shell(1, cx);
            assert_eq!(settings::get(cx).terminal_shell, "/bin/zsh");
            assert_eq!(
                terminal_shell_from(&settings::get(cx)),
                terminal_view::TerminalShell {
                    program: "/bin/zsh".to_string(),
                    args: vec!["-l".to_string()],
                },
                "新しく開く端末のシェルになる・引数を持たない行は引数に触らない（mac の従来どおり）"
            );
            workspace.commit_shell(2, cx);
            assert_eq!(
                terminal_shell_from(&settings::get(cx)),
                terminal_view::TerminalShell {
                    program: "wsl".to_string(),
                    args: vec!["-d".to_string(), "Ubuntu".to_string()],
                },
                "WSL はシェルと引数の組で書く"
            );
            workspace.commit_shell(0, cx);
            assert_eq!(settings::get(cx).terminal_shell, "");
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ターミナルの「▾」で選ぶと、頼んだドックに新しいタブが開き、タブ名はシェルの名前・設定は変えない。
    #[gpui::test]
    fn choosing_a_shell_from_the_terminal_menu_opens_a_new_tab(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_shell_menu_{}", std::process::id()));
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
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            let dock = workspace.terminal_dock.clone();
            dock.update(cx, |dock, _cx| dock.use_test_terminals());
            let wsl = ShellChoice {
                kind: ShellKind::Wsl {
                    distro: "Ubuntu".to_string(),
                },
                program: "wsl".to_string(),
                args: Some(vec!["-d".to_string(), "Ubuntu".to_string()]),
            };
            let title = wsl.label();
            workspace.overlays.picker_shells = vec![wsl];
            workspace.overlays.picker_shell_dock = Some(dock.downgrade());
            let before = dock.read(cx).tab_terminals().0.len();
            workspace.commit_new_terminal_shell(0, window, cx);
            let (terminals, active) = dock.read(cx).tab_terminals();
            assert_eq!(terminals.len(), before + 1, "新しいタブが開く");
            assert_eq!(
                terminals[active].read(cx).custom_title(),
                Some(title.as_str()),
                "タブ名はシェルの名前"
            );
            assert!(workspace.chrome.show_bottom, "下のドックを出す");
            assert_eq!(settings::get(cx).terminal_shell, "", "設定は変えない");
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Windows のシェル: PATH にある PowerShell 7 / Windows PowerShell / cmd、在る Git Bash 1 つ、
    /// WSL のディストリごとに 1 行。PATH の `bash`（System32 の WSL の入口）は Git Bash と取り違えない。
    #[test]
    fn windows_shells_list_powershell_git_bash_and_each_wsl_distro() {
        let in_path = ["powershell", "cmd", "bash", "wsl"];
        let find = |name: &str| {
            in_path
                .contains(&name)
                .then(|| PathBuf::from(format!(r"C:\Windows\System32\{name}.exe")))
        };
        let git_bash = [
            PathBuf::from(r"C:\Missing\Git\bin\bash.exe"),
            PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"),
        ];
        let exists = |path: &Path| path == Path::new(r"C:\Program Files\Git\bin\bash.exe");
        let distros = ["Ubuntu".to_string(), "Debian".to_string()];
        let shells = windows_shells(find, &git_bash, exists, &distros);
        let summary: Vec<(ShellKind, String)> = shells
            .iter()
            .map(|shell| (shell.kind.clone(), shell.detail()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (ShellKind::WindowsPowerShell, "powershell".to_string()),
                (ShellKind::Cmd, "cmd".to_string()),
                (
                    ShellKind::GitBash,
                    r"C:\Program Files\Git\bin\bash.exe --login -i".to_string()
                ),
                (
                    ShellKind::Wsl {
                        distro: "Ubuntu".to_string()
                    },
                    "wsl -d Ubuntu".to_string()
                ),
                (
                    ShellKind::Wsl {
                        distro: "Debian".to_string()
                    },
                    "wsl -d Debian".to_string()
                ),
            ],
            "pwsh が無ければ出さない・Git Bash は在る候補の先頭"
        );
        assert!(
            shells[0].is_current("powershell", &[]),
            "引数の無い PowerShell"
        );
        assert!(
            !shells[3].is_current("wsl", &["-d".to_string(), "Debian".to_string()]),
            "WSL は引数（ディストリ）まで見て今の行を決める"
        );
        assert!(shells[4].is_current("wsl", &["-d".to_string(), "Debian".to_string()]));
    }

    /// Git Bash の候補: PATH の git.exe（cmd / bin / mingw64\bin のどれでも）が居る Git を先に、
    /// 既定の置き場・Scoop を後に。大文字小文字だけ違う重複は外す。
    #[test]
    fn git_bash_is_found_next_to_git_on_path_first() {
        // 区切りはこの OS の物で組む（mac / Linux の CI でも `Path::parent` が同じように分かれる）。
        let path = |parts: &[&str]| parts.iter().collect::<PathBuf>();
        let program_dirs = [
            path(&["C:", "Program Files"]),
            path(&["C:", "Users", "a", "AppData", "Local", "Programs"]),
        ];
        let scoop = Some(path(&[
            "C:", "Users", "a", "scoop", "apps", "git", "current",
        ]));
        let expected = path(&["D:", "Tools", "Git", "bin", "bash.exe"]);
        for git in [
            path(&["D:", "Tools", "Git", "cmd", "git.exe"]),
            path(&["D:", "Tools", "Git", "bin", "git.exe"]),
            path(&["D:", "Tools", "Git", "mingw64", "bin", "git.exe"]),
        ] {
            let candidates = git_bash_candidates(Some(&git), &program_dirs, scoop.clone());
            assert_eq!(candidates.first(), Some(&expected), "{}", git.display());
        }
        let candidates = git_bash_candidates(
            Some(&path(&["C:", "PROGRAM FILES", "Git", "cmd", "git.exe"])),
            &program_dirs,
            scoop,
        );
        assert_eq!(
            candidates,
            vec![
                path(&["C:", "PROGRAM FILES", "Git", "bin", "bash.exe"]),
                path(&[
                    "C:", "Users", "a", "AppData", "Local", "Programs", "Git", "bin", "bash.exe"
                ]),
                path(&[
                    "C:", "Users", "a", "scoop", "apps", "git", "current", "bin", "bash.exe"
                ]),
            ],
            "Program Files の Git は PATH の物と同じなので重ねない"
        );
    }

    /// `wsl -l -q` は BOM の無い UTF-16LE（新しい版は WSL_UTF8=1 で UTF-8）。Docker の内部用は外す。
    #[test]
    fn wsl_distros_are_read_from_utf16_or_utf8() {
        let utf16: Vec<u8> = "Ubuntu-24.04\r\ndocker-desktop\r\nDebian\r\n\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            parse_wsl_distros(&utf16),
            vec!["Ubuntu-24.04".to_string(), "Debian".to_string()]
        );
        assert_eq!(
            parse_wsl_distros(b"Ubuntu\ndocker-desktop-data\n"),
            vec!["Ubuntu".to_string()]
        );
        assert!(parse_wsl_distros(b"").is_empty());
    }
}
