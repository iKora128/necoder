//! エージェントの起動の上書き（O16・B07）。設定 › AI エージェントの各行の下に「起動: …」の 1 行を出し、
//! 「変える…」で `agent_servers.<id>`（settings.json）を画面から書く。「既定に戻す」で項目ごと消す。
//!
//! 2 つの形（[`settings_core::AgentServerSetting`]）:
//! - **レジストリの起動に環境変数を足す**（`registry`）: 版とコマンドはレジストリのまま。
//! - **自分のコマンドで起動**（`custom`）: コマンド・引数（1 行に 1 つ）・環境変数を丸ごと決める。
//!
//! 値は settings.json に平文で残るので、API キーなどの秘密は書かない（案内を出す）。効くのは次に
//! 起動するエージェントから。
//!
//! 設定で足したエージェント（H1・組み込みでない id）も同じダイアログで変える。違いは 2 つ:
//! 「既定に戻す」の代わりに「外す」（`agent_servers.<id>` を消す＝一覧から消える）、保存しても
//! `name` を保つ（消すと一覧の名前が id に戻る）。自分のコマンドの物には「環境変数を足す」の形が無い
//! （戻る先のレジストリの起動が無い）。

use super::*;
use gpui::Focusable as _;
use settings_core::{env_lines, parse_arg_lines, parse_env_lines, AgentServerSetting};

/// 起動の上書きを編んでいるダイアログ。
pub(crate) struct LaunchEditor {
    pub(crate) agent_id: SharedString,
    pub(crate) agent_label: SharedString,
    /// 組み込みの 7 件か（`false` = 設定で足したエージェント）。
    pub(crate) builtin: bool,
    /// 足したエージェントの一覧の名前（保存しても保つ）。
    pub(crate) name: Option<String>,
    /// 「既定の起動に環境変数を足す」の形を選べるか（自分のコマンドで足した物は選べない）。
    pub(crate) registry_allowed: bool,
    /// `true` = 自分のコマンドで起動（`custom`）。
    pub(crate) custom: bool,
    pub(crate) command: Entity<EditorView>,
    pub(crate) args: Entity<EditorView>,
    pub(crate) env: Entity<EditorView>,
    pub(crate) error: Option<SharedString>,
}

/// `agent_servers.<id>` を丸ごと書き換える（`None` = 既定の起動に戻す）。**即適用 + 永続化**。
pub fn set_agent_server(
    cx: &mut App,
    agent_id: &str,
    setting: Option<&AgentServerSetting>,
) -> anyhow::Result<()> {
    write_user_file(cx, |path| {
        settings_core::persist_agent_server(path, agent_id, setting)
    })
}

/// 行に出す今の起動（既定 / 足した環境変数 / 自分のコマンド）。
pub(crate) fn launch_summary(setting: Option<&AgentServerSetting>) -> String {
    match setting {
        None => i18n::t!("settings.launch_default"),
        Some(AgentServerSetting::Registry { env, .. }) if env.is_empty() => {
            i18n::t!("settings.launch_default")
        }
        Some(AgentServerSetting::Registry { env, .. }) => i18n::t!(
            "settings.launch_env",
            "names" => env.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
        Some(AgentServerSetting::Custom { command, args, .. }) => {
            let mut line = command.clone();
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            i18n::t!("settings.launch_custom", "command" => line)
        }
    }
}

impl SettingsView {
    /// 「変える…」: 今の設定を欄に入れてダイアログを開く（環境変数の欄へフォーカス）。
    pub(crate) fn open_launch_editor(
        &mut self,
        agent_id: SharedString,
        agent_label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = get(cx).agent_servers.get(agent_id.as_ref()).cloned();
        let builtin = acp_client::AGENTS
            .iter()
            .any(|agent| agent.id == agent_id.as_ref());
        let name = current
            .as_ref()
            .and_then(AgentServerSetting::name)
            .map(str::to_string)
            .filter(|_| !builtin);
        // 自分のコマンドで足した物には戻る先（レジストリの起動）が無い。
        let registry_allowed =
            builtin || !matches!(current, Some(AgentServerSetting::Custom { .. }));
        let theme = self.theme.clone();
        let accent = self.accent;
        let field = |text: String, submit_on_enter: bool, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut view = EditorView::plain(theme.clone(), accent, submit_on_enter, cx);
                view.set_plain_text(&text, cx);
                view
            })
        };
        let (custom, command, args) = match &current {
            Some(AgentServerSetting::Custom { command, args, .. }) => {
                (true, command.clone(), args.join("\n"))
            }
            _ => (false, String::new(), String::new()),
        };
        let env = current
            .as_ref()
            .map(|setting| env_lines(setting.env()))
            .unwrap_or_default();
        let command = field(command, true, cx);
        cx.subscribe(&command, |view, _command, event, cx| {
            if matches!(event, editor_view::ComposerEvent::Submit) {
                view.save_launch_editor(cx);
            }
        })
        .detach();
        let args = field(args, false, cx);
        let env = field(env, false, cx);
        window.focus(&env.read(cx).focus_handle(cx), cx);
        self.launch_editor = Some(LaunchEditor {
            agent_id,
            agent_label,
            builtin,
            name,
            registry_allowed,
            custom,
            command,
            args,
            env,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_launch_editor(&mut self, cx: &mut Context<Self>) {
        if self.launch_editor.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn set_launch_custom(&mut self, custom: bool, cx: &mut Context<Self>) {
        if let Some(editor) = self.launch_editor.as_mut() {
            editor.custom = custom || !editor.registry_allowed;
            editor.error = None;
            cx.notify();
        }
    }

    /// 欄から設定を作る（`None` = 何も足さない＝既定の起動）。誤りは理由。
    /// 足したエージェントは空の環境変数でも項目を残す（消すと一覧から消える）。
    fn launch_editor_setting(&self, cx: &App) -> Result<Option<AgentServerSetting>, String> {
        let editor = self.launch_editor.as_ref().ok_or_else(String::new)?;
        let env = parse_env_lines(&editor.env.read(cx).plain_text())?;
        let name = editor.name.clone();
        if editor.custom {
            let command = editor.command.read(cx).plain_text().trim().to_string();
            if command.is_empty() {
                return Err(i18n::t!("settings.launch_command_missing"));
            }
            let args = parse_arg_lines(&editor.args.read(cx).plain_text());
            return Ok(Some(AgentServerSetting::Custom {
                name,
                command,
                args,
                env,
            }));
        }
        Ok((!editor.builtin || !env.is_empty())
            .then_some(AgentServerSetting::Registry { name, env }))
    }

    /// 「保存」: 確かめて settings.json に書く。書けたら閉じる。
    pub(crate) fn save_launch_editor(&mut self, cx: &mut Context<Self>) {
        let setting = self.launch_editor_setting(cx);
        let Some(agent_id) = self
            .launch_editor
            .as_ref()
            .map(|editor| editor.agent_id.clone())
        else {
            return;
        };
        let result = setting.and_then(|setting| {
            set_agent_server(cx, &agent_id, setting.as_ref())
                .map_err(|error| save_failure_message(&error).to_string())
        });
        match result {
            Ok(()) => self.launch_editor = None,
            Err(error) => {
                if let Some(editor) = self.launch_editor.as_mut() {
                    editor.error = Some(SharedString::from(error));
                }
            }
        }
        cx.notify();
    }

    /// 行の「既定に戻す」: `agent_servers.<id>` を消す。
    pub(crate) fn reset_agent_launch(&mut self, agent_id: &str, cx: &mut Context<Self>) {
        let result = set_agent_server(cx, agent_id, None);
        self.report_save(result, cx);
        cx.notify();
    }

    /// 足したエージェントの「外す」: `agent_servers.<id>` を消す（一覧から消える）。既定のエージェントと
    /// Captain は外せない（使う / 使わないのスイッチと同じ理由・先に別のエージェントを既定にする）。
    pub(crate) fn remove_custom_agent(
        &mut self,
        agent_id: &str,
        locked: bool,
        cx: &mut Context<Self>,
    ) {
        if locked {
            cx.emit(SettingsViewEvent::SaveFailed(SharedString::from(i18n::t!(
                "settings.agent_disable_locked"
            ))));
            return;
        }
        let result = set_agent_server(cx, agent_id, None);
        self.report_save(result, cx);
        cx.notify();
    }

    /// 1 エージェント分の起動の行: 「起動: 今の起動」+「変える…」（+ 上書きがあれば「既定に戻す」）。
    /// 足したエージェント（`removable`）は「既定に戻す」の代わりに「外す」を出す。`locked` = 外せない
    /// （既定のエージェント・Captain）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn launch_line(
        &self,
        index: usize,
        agent_id: SharedString,
        agent_label: SharedString,
        removable: bool,
        locked: bool,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let current = settings.agent_servers.get(agent_id.as_ref());
        let link = |id: (&'static str, usize), label: String| {
            div()
                .id(id)
                .flex_none()
                .px(px(6.))
                .text_size(px(11.))
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.fg0))
                .child(SharedString::from(label))
        };
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .pl(px(48.))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(launch_summary(current))),
            )
            .child({
                let (agent_id, agent_label) = (agent_id.clone(), agent_label.clone());
                link(("launch-edit", index), i18n::t!("settings.launch_edit")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, window, cx| {
                        view.open_launch_editor(agent_id.clone(), agent_label.clone(), window, cx)
                    }),
                )
            })
            .when(!removable && current.is_some(), |line| {
                let agent_id = agent_id.clone();
                line.child(
                    link(("launch-reset", index), i18n::t!("settings.launch_reset")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.reset_agent_launch(&agent_id, cx)
                        }),
                    ),
                )
            })
            .when(removable, |line| {
                line.child(
                    link(("agent-remove", index), i18n::t!("settings.agent_remove"))
                        .when(locked, |link| link.opacity(0.45))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, _window, cx| {
                                view.remove_custom_agent(&agent_id, locked, cx)
                            }),
                        ),
                )
            })
    }

    /// 起動の上書きのダイアログ（設定の面の上・中央・幅 520）。
    pub(crate) fn render_launch_editor(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let editor = self.launch_editor.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent;
        let chip = |id: &'static str, label: String, chosen: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(9.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(11.5))
                .cursor_pointer()
                .when(chosen, |chip| chip.bg(theme.bg3).text_color(theme.fg0))
                .when(!chosen, |chip| {
                    chip.border_1()
                        .border_color(theme.border)
                        .text_color(theme.fg1)
                        .hover(|style| style.text_color(theme.fg0))
                })
                .child(SharedString::from(label))
        };
        let field = |label: String, input: Entity<EditorView>, height: f32| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .h(px(height))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg0)
                        .overflow_hidden()
                        .child(input),
                )
        };
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .px(px(13.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(if primary { accent } else { theme.border })
                .bg(if primary {
                    accent.alpha(0.16)
                } else {
                    theme.bg1
                })
                .text_size(px(12.))
                .text_color(if primary { accent } else { theme.fg1 })
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3))
                .child(SharedString::from(label))
        };
        let custom = editor.custom;
        let card = div()
            .w(px(520.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(10.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(12.),
                gpui::hsla(0., 0., 0., 0.5),
            )
            .blur_radius(px(28.))])
            .on_key_down(
                cx.listener(|view, event: &gpui::KeyDownEvent, _window, cx| {
                    if event.keystroke.key.as_str() == "escape" {
                        view.close_launch_editor(cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!(
                        "settings.launch_title",
                        "agent" => editor.agent_label.as_ref()
                    ))),
            )
            .when(editor.registry_allowed, |card| {
                card.child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .child(
                            chip(
                                "launch-registry",
                                i18n::t!("settings.launch_mode_registry"),
                                !custom,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| {
                                    view.set_launch_custom(false, cx)
                                }),
                            ),
                        )
                        .child(
                            chip(
                                "launch-custom",
                                i18n::t!("settings.launch_mode_custom"),
                                custom,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| {
                                    view.set_launch_custom(true, cx)
                                }),
                            ),
                        ),
                )
            })
            .when(custom, |card| {
                card.child(field(
                    i18n::t!("settings.launch_command"),
                    editor.command.clone(),
                    30.,
                ))
                .child(field(
                    i18n::t!("settings.launch_args"),
                    editor.args.clone(),
                    64.,
                ))
            })
            .child(field(
                i18n::t!("settings.launch_env_field"),
                editor.env.clone(),
                88.,
            ))
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.launch_note"))),
            )
            .when_some(editor.error.clone(), |card, error| {
                card.child(div().text_size(px(11.5)).text_color(theme.err).child(error))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("launch-cancel", i18n::t!("settings.launch_cancel"), false)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| view.close_launch_editor(cx)),
                            ),
                    )
                    .child(
                        button("launch-save", i18n::t!("settings.launch_save"), true)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| view.save_launch_editor(cx)),
                            ),
                    ),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.25))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, _, _window, cx| view.close_launch_editor(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}
