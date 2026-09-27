//! どこからでも呼べる端末（O24・C13）。プロジェクトに紐付かない端末を、エディタ・Fleet・Chat のどの
//! 画面でも前面に浮かべる（パレット「ターミナル: どこからでも（浮かべる）」・⌃`）。
//!
//! - 中身は下ドックと同じ `TerminalDock`（タブ・横の分割・⌘T / ⌘W・よく使うコマンド）を窓に 1 つだけ
//!   持ち、ホームのフォルダで開く（どのプロジェクトの中でもない）。初めて呼ぶまでシェルは起こさない。
//! - 隠しても止めない（もう一度呼ぶと同じシェル）。最後の端末を閉じると隠れ、次は新しいシェル。
//! - ⌃` は、隠れていれば出してフォーカス、出ていてフォーカスが外なら中へ、中なら隠して元の所へ返す。
//! - 窓を閉じる・終了する時の確認（O4）はほかの端末と同じく数える。テーマは窓と一緒に変わる。

use crate::workspace::*;

/// 窓に 1 つの浮かべた端末。
pub(crate) struct FloatingTerminal {
    pub(crate) dock: Entity<TerminalDock>,
    pub(crate) visible: bool,
    /// 出す前にフォーカスがあった所（隠したら返す）。
    previous_focus: Option<FocusHandle>,
    _events: Subscription,
}

impl Workspace {
    /// ⌃` / パレット「ターミナル: どこからでも（浮かべる）」。
    pub(crate) fn toggle_floating_terminal(
        &mut self,
        _: &ToggleFloatingTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused_inside = self
            .chrome
            .floating_terminal
            .as_ref()
            .is_some_and(|floating| {
                floating.visible && floating.dock.read(cx).contains_focus(window, cx)
            });
        if focused_inside {
            self.hide_floating_terminal(window, cx);
        } else {
            self.show_floating_terminal(window, cx);
        }
    }

    /// 浮かべる端末の `TerminalDock`（無ければ作る・シェルはまだ起こさない）。
    pub(crate) fn floating_terminal_dock(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalDock> {
        if let Some(floating) = &self.chrome.floating_terminal {
            return floating.dock.clone();
        }
        // ホームで開く。Windows の既定シェル（pwsh を優先・§W4）は手元のプロジェクトの端末と同じ決め方。
        let home = paths::home_dir();
        let shell = home
            .as_deref()
            .and_then(|home| {
                host::LocalHost::shared()
                    .terminal_launch(home)
                    .ok()
                    .flatten()
            })
            .map(|launch| (launch.program, launch.args));
        let launch = TerminalLaunch { cwd: home, shell };
        let theme = self.theme.clone();
        let accent = self.theme.fg2;
        let key = Self::shortcut_label_for("workspace::ToggleFloatingTerminal").unwrap_or_default();
        let tip = i18n::t!("terminal.float_hide_tip", "key" => key);
        let dock = cx.new(|_| {
            TerminalDock::new(launch, theme)
                .with_accent(accent)
                .with_close_tip(tip)
        });
        let events = cx.subscribe(&dock, Self::on_floating_terminal_event);
        self.chrome.floating_terminal = Some(FloatingTerminal {
            dock: dock.clone(),
            visible: false,
            previous_focus: None,
            _events: events,
        });
        dock
    }

    /// 出して中へフォーカス（端末が無ければここで初めてシェルを起こす）。
    pub(crate) fn show_floating_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dock = self.floating_terminal_dock(cx);
        let previous = window.focused(cx);
        if let Some(floating) = self.chrome.floating_terminal.as_mut() {
            floating.visible = true;
            floating.previous_focus = previous;
        }
        dock.update(cx, |dock, cx| dock.focus_active(window, cx));
        cx.notify();
    }

    /// 隠す（止めない）。フォーカスは出す前の所へ返す（無ければ窓へ・隠れた端末に打鍵を残さない）。
    pub(crate) fn hide_floating_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fallback = self.focus_handle.clone();
        let Some(floating) = self.chrome.floating_terminal.as_mut() else {
            return;
        };
        floating.visible = false;
        let previous = floating.previous_focus.take().unwrap_or(fallback);
        window.focus(&previous, cx);
        cx.notify();
    }

    /// 浮かべた端末からの知らせ。パスはホームから、URL は手元の物として開く（プロジェクトの端末と違い、
    /// SSH 先の localhost ではない）。最後の端末を閉じたら隠す。
    fn on_floating_terminal_event(
        &mut self,
        _dock: Entity<TerminalDock>,
        event: &TerminalDockEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalDockEvent::OpenPath { path, line } => {
                let resolved = if Path::new(path).is_absolute() {
                    PathBuf::from(path)
                } else {
                    let Some(home) = paths::home_dir() else {
                        return;
                    };
                    home.join(path.strip_prefix("~/").unwrap_or(path))
                };
                self.pending_navigation = Some((resolved, line.saturating_sub(1) as usize, 0));
            }
            TerminalDockEvent::OpenUrl(url) => self.open_url(url, cx),
            TerminalDockEvent::Dismissed => {
                // 窓を持たない（イベント）ので、フォーカスは次の描画で返す。
                let fallback = self.focus_handle.clone();
                if let Some(floating) = self.chrome.floating_terminal.as_mut() {
                    floating.visible = false;
                    self.chrome.focus_next_frame =
                        Some(floating.previous_focus.take().unwrap_or(fallback));
                }
            }
            TerminalDockEvent::RenameRequested(terminal) => {
                self.start_terminal_rename(terminal.clone(), cx)
            }
        }
        cx.notify();
    }

    /// 浮かべた端末（画面の下寄りの中央・幅 76%・高さ 48%）。パレットやダイアログはこの上に出る。
    /// 後ろの画面へはクリックを通さない（`occlude`）が、外を押しても隠れない（⌃` か × で隠す）。
    pub(crate) fn render_floating_terminal(
        &self,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let floating = self
            .chrome
            .floating_terminal
            .as_ref()
            .filter(|floating| floating.visible)?;
        let theme = &self.theme;
        Some(
            div()
                .id("floating-terminal")
                .absolute()
                .left(gpui::relative(0.12))
                .w(gpui::relative(0.76))
                .bottom(px(40.))
                .h(gpui::relative(0.48))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .overflow_hidden()
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg1)
                .shadow(vec![gpui::BoxShadow::new(
                    px(0.),
                    px(10.),
                    gpui::hsla(0., 0., 0., 0.45),
                )
                .blur_radius(px(28.))])
                .occlude()
                .child(floating.dock.clone())
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O24・C13: ⌃` で出してフォーカス・もう一度で隠して元へ・また出すと同じシェル。最後の端末を
    /// 閉じると隠れる。PTY は起こさない（テスト用の端末）。
    #[gpui::test]
    fn a_floating_terminal_comes_up_anywhere_and_keeps_its_shell(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_floating_terminal_{}", std::process::id()));
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
        let first = workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            let dock = workspace.floating_terminal_dock(cx);
            dock.update(cx, |dock, _cx| dock.use_test_terminals());
            workspace.toggle_floating_terminal(&ToggleFloatingTerminal, window, cx);
            let floating = workspace.chrome.floating_terminal.as_ref().expect("作った");
            assert!(floating.visible);
            assert!(dock.read(cx).contains_focus(window, cx), "出したら中へ");
            let first = dock.read(cx).active_terminal().expect("端末").entity_id();

            workspace.toggle_floating_terminal(&ToggleFloatingTerminal, window, cx);
            assert!(!workspace.chrome.floating_terminal.as_ref().unwrap().visible);
            assert!(
                !dock.read(cx).contains_focus(window, cx),
                "隠したら外へ返す"
            );

            // Fleet でも出る（どの画面でも同じ 1 つ）。止めていないので同じシェル。
            workspace.chrome.fleet_mode = true;
            workspace.toggle_floating_terminal(&ToggleFloatingTerminal, window, cx);
            assert_eq!(
                dock.read(cx)
                    .active_terminal()
                    .map(|terminal| terminal.entity_id()),
                Some(first)
            );
            assert_eq!(
                workspace.running_work(cx).terminals,
                0,
                "動いていなければ数えない"
            );
            first
        });
        // 描けること。中の ⌘W で最後の端末を閉じると隠れる。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.dispatch_action(terminal_view::actions::CloseTab);
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            let floating = workspace.chrome.floating_terminal.as_ref().expect("残る");
            assert!(!floating.visible, "最後の端末を閉じたら隠れる");
            assert!(floating.dock.read(cx).active_terminal().is_none());
            // 次に出すと新しいシェル。
            workspace.show_floating_terminal(window, cx);
            let dock = workspace
                .chrome
                .floating_terminal
                .as_ref()
                .unwrap()
                .dock
                .clone();
            assert_ne!(
                dock.read(cx)
                    .active_terminal()
                    .map(|terminal| terminal.entity_id()),
                Some(first)
            );
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
