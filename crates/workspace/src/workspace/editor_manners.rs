//! エディタの所作（O26）: `path:行` のコピー（⌘⌥C）・タブを全部閉じる（⌘K ⌘W）・外部のエディタ /
//! ターミナルで開く（パレット）。
//!
//! 外部アプリは**手元のプロジェクトだけ**（SSH 先のファイルは手元のアプリで開けない）。VS Code /
//! Cursor / Zed は PATH の CLI（`code` / `cursor` / `zed`）で行まで渡す。CLI が無い macOS は
//! `open -a <アプリ>` でファイルだけ開く。どれも無ければトーストで知らせる。

use crate::workspace::*;

/// 外部で開く先。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalApp {
    VsCode,
    Cursor,
    Zed,
    Terminal,
}

impl ExternalApp {
    /// PATH で探す CLI。ターミナルは OS ごとに別（[`terminal_command`]）。
    fn cli(self) -> Option<&'static str> {
        match self {
            Self::VsCode => Some("code"),
            Self::Cursor => Some("cursor"),
            Self::Zed => Some("zed"),
            Self::Terminal => None,
        }
    }

    /// macOS のアプリ名（CLI が無い時の `open -a`）。
    fn mac_app(self) -> &'static str {
        match self {
            Self::VsCode => "Visual Studio Code",
            Self::Cursor => "Cursor",
            Self::Zed => "Zed",
            Self::Terminal => "Terminal",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::VsCode => "VS Code",
            Self::Cursor => "Cursor",
            Self::Zed => "Zed",
            Self::Terminal => "Terminal",
        }
    }
}

/// エディタの CLI に渡す引数（純関数）。プロジェクトのフォルダを開き、ファイルは行まで渡す
/// （VS Code / Cursor は `--goto path:行`、Zed は `path:行`）。ファイルが無ければフォルダだけ。
pub(crate) fn editor_cli_args(
    app: ExternalApp,
    folder: &Path,
    file: Option<(&Path, usize)>,
) -> Vec<String> {
    let mut args = vec![folder.display().to_string()];
    if let Some((file, line)) = file {
        let target = format!("{}:{line}", file.display());
        match app {
            ExternalApp::VsCode | ExternalApp::Cursor => {
                args.push("--goto".to_string());
                args.push(target);
            }
            ExternalApp::Zed => args.push(target),
            ExternalApp::Terminal => {}
        }
    }
    args
}

/// `path:行` / `path:10-14`（プロジェクト相対・純関数）。transcript のリンク検出でそのまま開ける形。
pub(crate) fn path_with_lines(relative: &Path, start: usize, end: usize) -> String {
    if start == end {
        format!("{}:{start}", relative.display())
    } else {
        format!("{}:{start}-{end}", relative.display())
    }
}

/// OS のターミナルでフォルダを開くコマンド（無い OS は `None`）。
fn terminal_command(folder: &Path) -> Option<(String, Vec<String>)> {
    let folder = folder.display().to_string();
    if cfg!(target_os = "macos") {
        Some((
            "open".to_string(),
            vec!["-a".to_string(), "Terminal".to_string(), folder],
        ))
    } else if cfg!(target_os = "windows") {
        if host::find_in_path("wt").is_some() {
            Some(("wt".to_string(), vec!["-d".to_string(), folder]))
        } else {
            Some((
                "cmd".to_string(),
                vec![
                    "/c".to_string(),
                    "start".to_string(),
                    "cmd".to_string(),
                    "/k".to_string(),
                    format!("cd /d \"{folder}\""),
                ],
            ))
        }
    } else {
        host::find_in_path("x-terminal-emulator").map(|terminal| {
            (
                terminal.display().to_string(),
                vec![format!("--working-directory={folder}")],
            )
        })
    }
}

impl Workspace {
    /// ⌘⌥C: 開いているファイルの `path:行`（選んでいれば `path:10-14`）をコピー。
    pub(crate) fn copy_path_with_line(
        &mut self,
        _: &CopyPathWithLine,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let (path, (start, end)) = {
            let editor = editor.read(cx);
            let Some(path) = editor.buffer().path().map(Path::to_path_buf) else {
                return;
            };
            (path, editor.primary_line_range())
        };
        let relative = self
            .active_slot()
            .and_then(|slot| path.strip_prefix(slot.worktree.root()).ok())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.clone());
        let text = path_with_lines(&relative, start, end);
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        let color = self.accent();
        self.push_toast(
            i18n::t!("tabs.copied_path_line", "path" => text).into(),
            color,
            cx,
        );
    }

    /// ⌘K ⌘W: タブを全部閉じる。**未保存のタブは残す**（閉じると編集を捨てる仕様なので、まとめて
    /// 閉じる操作では失わせない）。残した枚数はトーストで知らせる。
    pub(crate) fn close_all_tabs(
        &mut self,
        _: &CloseAllTabs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_saved_tabs(window, cx);
    }

    /// 保存済みのタブを全部閉じる（タブメニュー「全部閉じる」と ⌘K ⌘W の本体）。
    pub(crate) fn close_saved_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut kept = 0;
        for index in (0..self.tabs.len()).rev() {
            if self.tabs[index].is_dirty(cx) {
                kept += 1;
                continue;
            }
            self.close_tab_at(index, window, cx);
        }
        if kept > 0 {
            let color = self.accent();
            self.push_toast(
                i18n::t!("tabs.kept_unsaved", "count" => kept).into(),
                color,
                cx,
            );
        }
    }

    pub(crate) fn open_in_vs_code(
        &mut self,
        _: &OpenInVsCode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_in_external_app(ExternalApp::VsCode, cx);
    }

    pub(crate) fn open_in_cursor(
        &mut self,
        _: &OpenInCursor,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_in_external_app(ExternalApp::Cursor, cx);
    }

    pub(crate) fn open_in_zed(
        &mut self,
        _: &OpenInZed,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_in_external_app(ExternalApp::Zed, cx);
    }

    pub(crate) fn open_in_terminal(
        &mut self,
        _: &OpenInTerminal,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_in_external_app(ExternalApp::Terminal, cx);
    }

    /// 開いているファイル（無ければプロジェクトのフォルダ）を外部アプリで開く。
    pub(crate) fn open_in_external_app(&mut self, app: ExternalApp, cx: &mut Context<Self>) {
        let color = self.accent();
        let Some(slot) = self.active_slot() else {
            return;
        };
        if slot.worktree.is_remote() {
            self.push_toast(i18n::t!("tabs.external_remote").into(), color, cx);
            return;
        }
        let folder = slot.worktree.root().to_path_buf();
        let file = self.active_editor().and_then(|editor| {
            let editor = editor.read(cx);
            let path = editor.buffer().path()?.to_path_buf();
            Some((path, editor.primary_line_range().0))
        });
        let command = match app.cli() {
            Some(cli) => match host::find_in_path(cli) {
                Some(program) => Some((
                    program.display().to_string(),
                    editor_cli_args(
                        app,
                        &folder,
                        file.as_ref().map(|(path, line)| (path.as_path(), *line)),
                    ),
                )),
                // CLI が無い macOS は、アプリにファイル（無ければフォルダ）だけ渡す。
                None if cfg!(target_os = "macos") => Some((
                    "open".to_string(),
                    vec![
                        "-a".to_string(),
                        app.mac_app().to_string(),
                        file.as_ref()
                            .map_or(folder.as_path(), |(path, _)| path.as_path())
                            .display()
                            .to_string(),
                    ],
                )),
                None => None,
            },
            None => terminal_command(&folder),
        };
        let Some((program, args)) = command else {
            self.push_toast(
                i18n::t!("tabs.external_missing", "app" => app.label()).into(),
                color,
                cx,
            );
            return;
        };
        match std::process::Command::new(&program).args(&args).spawn() {
            // 終わるのを背景で待つ（終わった子プロセスを残さない）。
            Ok(mut child) => cx
                .background_executor()
                .spawn(async move {
                    if let Err(error) = child.wait() {
                        eprintln!("外部アプリの終了を待てない: {error}");
                    }
                })
                .detach(),
            Err(error) => {
                eprintln!("{program} を起動できない: {error}");
                self.push_toast(
                    i18n::t!("tabs.external_missing", "app" => app.label()).into(),
                    color,
                    cx,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editors_get_the_folder_and_the_file_with_its_line() {
        let folder = Path::new("/work/necoder");
        let file = Path::new("/work/necoder/src/main.rs");
        assert_eq!(
            editor_cli_args(ExternalApp::VsCode, folder, Some((file, 12))),
            vec!["/work/necoder", "--goto", "/work/necoder/src/main.rs:12"]
        );
        assert_eq!(
            editor_cli_args(ExternalApp::Cursor, folder, Some((file, 3))),
            vec!["/work/necoder", "--goto", "/work/necoder/src/main.rs:3"]
        );
        assert_eq!(
            editor_cli_args(ExternalApp::Zed, folder, Some((file, 7))),
            vec!["/work/necoder", "/work/necoder/src/main.rs:7"]
        );
        assert_eq!(
            editor_cli_args(ExternalApp::Zed, folder, None),
            vec!["/work/necoder"],
            "ファイルが無ければフォルダだけ"
        );
    }

    #[test]
    fn paths_with_lines_are_transcript_links() {
        assert_eq!(path_with_lines(Path::new("src/a.rs"), 4, 4), "src/a.rs:4");
        assert_eq!(
            path_with_lines(Path::new("src/a.rs"), 10, 14),
            "src/a.rs:10-14"
        );
        let text = path_with_lines(Path::new("src/a.rs"), 10, 14);
        let linked = ui::links::find_links(&text).into_iter().any(|link| {
            matches!(link.target, ui::links::LinkTarget::Path { ref path, line: Some(10), .. } if path == "src/a.rs")
        });
        assert!(linked, "範囲は先頭の行へのリンクになる");
    }

    /// ⌘K ⌘W は保存済みのタブだけ閉じ、未保存のタブは残して知らせる。
    #[gpui::test]
    fn closing_all_tabs_keeps_unsaved_ones(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!("necoder_close_all_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(project.join(name), format!("{name}\n")).unwrap();
        }
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            for name in ["a.txt", "b.txt", "c.txt"] {
                workspace.open_file(project.join(name), window, cx);
            }
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(workspace.tabs.len(), 3);
            // b.txt だけ編集して未保存にする。
            let edited = workspace
                .tabs
                .iter()
                .position(|tab| tab.path.ends_with("b.txt"))
                .expect("b.txt のタブ");
            let editor = workspace.tabs[edited]
                .editor()
                .cloned()
                .expect("エディタのタブ");
            editor.update(cx, |editor, cx| editor.insert_text("edited ", cx));
            assert!(workspace.tabs[edited].is_dirty(cx));
            workspace.close_saved_tabs(window, cx);
            assert_eq!(workspace.tabs.len(), 1, "未保存の 1 枚だけ残る");
            assert!(workspace.tabs[0].path.ends_with("b.txt"));
            let toast = workspace
                .notifications
                .toasts
                .last()
                .map(|toast| toast.text.to_string())
                .unwrap_or_default();
            assert!(toast.contains('1'), "残した枚数を知らせる: {toast}");
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
