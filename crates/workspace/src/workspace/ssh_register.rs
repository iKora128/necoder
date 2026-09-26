//! 接続先の登録（O37・G01）。SSH のホストピッカーの「＋ 接続先を登録…」とパレットから、名前・ホスト名・
//! ユーザー・ポート・鍵のファイルを入れて SSH config（`~/.ssh/config`）の末尾に `Host` の塊を足す。
//! 既存の行は触らず、同じ名前は断る（`host::append_ssh_config_host`）。
//!
//! necoder は SSH config を正にしている（ProxyJump・鍵・多重化はそのまま効く）ので、登録した接続先は
//! ssh / scp / ほかのエディタからも使える。足したら、その場で接続を確かめる（パレットの接続テストと同じ）。

use crate::workspace::*;

/// 入力欄の並び（見出し・例の i18n キー）。値の読み方は [`Workspace::ssh_register_values`]。
const FIELDS: [(&str, &str); 5] = [
    ("ssh.register_alias", "ssh.register_alias_hint"),
    ("ssh.register_hostname", "ssh.register_hostname_hint"),
    ("ssh.register_user", "ssh.register_user_hint"),
    ("ssh.register_port", "ssh.register_port_hint"),
    ("ssh.register_identity", "ssh.register_identity_hint"),
];

/// 登録のダイアログ。
pub(crate) struct SshRegistering {
    /// 名前・ホスト名・ユーザー・ポート・鍵のファイル（[`FIELDS`] の順）。
    pub(crate) fields: [Entity<EditorView>; 5],
    /// 足す先（開いた時の `host::ssh_config_path()`・テストは一時フォルダ）。
    pub(crate) config_path: Option<PathBuf>,
    /// 足した後に接続を確かめるか（テストは実の ssh を起こさない）。
    pub(crate) test_after: bool,
    /// 書けなかった理由（入力の誤り・同じ名前・書けない）。
    pub(crate) error: Option<SharedString>,
}

impl Workspace {
    /// ダイアログを開く（名前の欄から）。
    pub(crate) fn open_ssh_register(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let accent = self.accent();
        let theme = self.theme.clone();
        let fields: [Entity<EditorView>; 5] = std::array::from_fn(|_| {
            cx.new(|cx| EditorView::plain(theme.clone(), accent, true, cx))
        });
        for field in &fields {
            cx.subscribe(field, |workspace, _field, event, cx| match event {
                ComposerEvent::Submit => workspace.confirm_ssh_register(cx),
                // 1 行の欄なので高さの追従は要らない。
                ComposerEvent::ContentHeightChanged => {}
            })
            .detach();
        }
        window.focus(&fields[0].read(cx).focus_handle(cx), cx);
        self.chrome.ssh_registering = Some(SshRegistering {
            fields,
            config_path: host::ssh_config_path(),
            test_after: true,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn cancel_ssh_register(&mut self, cx: &mut Context<Self>) {
        if self.chrome.ssh_registering.take().is_some() {
            cx.notify();
        }
    }

    /// 欄の値から足す物を作る（空の任意欄は書かない・ポートは数）。
    fn ssh_register_values(&self, cx: &App) -> Result<host::NewSshHost, String> {
        let registering = self
            .chrome
            .ssh_registering
            .as_ref()
            .ok_or_else(String::new)?;
        let [alias, hostname, user, port, identity] = registering
            .fields
            .each_ref()
            .map(|field| field.read(cx).plain_text().trim().to_string());
        let optional = |value: String| (!value.is_empty()).then_some(value);
        let port = match port.as_str() {
            "" => None,
            text => Some(
                text.parse::<u16>()
                    .map_err(|_| i18n::t!("ssh.register_bad_port", "port" => text))?,
            ),
        };
        let host = host::NewSshHost {
            alias,
            hostname,
            user: optional(user),
            port,
            identity_file: optional(identity),
        };
        host.validate().map_err(|error| format!("{error:#}"))?;
        Ok(host)
    }

    /// 「登録」（どの欄でも Enter）: 確かめて config に足す。足せたら閉じて知らせ、接続を確かめる。
    pub(crate) fn confirm_ssh_register(&mut self, cx: &mut Context<Self>) {
        let values = self.ssh_register_values(cx);
        let Some(registering) = self.chrome.ssh_registering.as_mut() else {
            return;
        };
        let result = values.and_then(|host| {
            let path = registering
                .config_path
                .clone()
                .ok_or_else(|| i18n::t!("ssh.register_no_config"))?;
            host::append_ssh_config_host(&path, &host)
                .map(|()| (host, path))
                .map_err(|error| format!("{error:#}"))
        });
        match result {
            Ok((host, path)) => {
                let test_after = registering.test_after;
                self.chrome.ssh_registering = None;
                let color = self.accent();
                self.push_toast(
                    SharedString::from(i18n::t!(
                        "ssh.registered",
                        "alias" => host.alias,
                        "path" => path.display()
                    )),
                    color,
                    cx,
                );
                if test_after {
                    self.test_ssh_uri(format!("ssh://{}", host.alias), cx);
                }
            }
            Err(error) => registering.error = Some(SharedString::from(error)),
        }
        cx.notify();
    }

    /// Tab / ⇧Tab で次 / 前の欄へ（1 行の欄に空白を入れない）。
    fn focus_next_ssh_field(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(registering) = self.chrome.ssh_registering.as_ref() else {
            return;
        };
        let current = registering
            .fields
            .iter()
            .position(|field| field.read(cx).focus_handle(cx).is_focused(window))
            .unwrap_or(0);
        let count = registering.fields.len();
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        let handle = registering.fields[next].read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// 登録のダイアログ（中央・幅 460）。書く塊をその場で見せる。
    pub(crate) fn render_ssh_register(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let registering = self.chrome.ssh_registering.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let preview = self
            .ssh_register_values(cx)
            .ok()
            .map(|host| host.config_block().trim().to_string());
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
        let mut form = div().flex().flex_col().gap(px(8.));
        for (index, (label, hint)) in FIELDS.iter().enumerate() {
            form = form.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(104.))
                            .pt(px(7.))
                            .text_size(px(12.))
                            .text_color(theme.fg1)
                            .child(SharedString::from(i18n::t!(label))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .h(px(30.))
                                    .px(px(8.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.bg0)
                                    .overflow_hidden()
                                    .child(registering.fields[index].clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(i18n::t!(hint))),
                            ),
                    ),
            );
        }
        let card = div()
            .w(px(460.))
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
            // 1 行の欄では Tab / ⇧Tab を欄の移動にする（編集の Tab = 空白を入れる、より先に受ける）。
            .capture_action(cx.listener(|this, _: &editor_view::TabIndent, window, cx| {
                this.focus_next_ssh_field(true, window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &editor_view::Outdent, window, cx| {
                this.focus_next_ssh_field(false, window, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    this.cancel_ssh_register(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _window, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(i18n::t!("ssh.register_title"))),
                    )
                    .child(div().text_size(px(11.)).text_color(theme.fg2).child(
                        SharedString::from(i18n::t!(
                            "ssh.register_sub",
                            "path" => registering
                                .config_path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "~/.ssh/config".to_string())
                        )),
                    )),
            )
            .child(form)
            .when_some(preview, |card, preview| {
                card.child(
                    div()
                        .p(px(8.))
                        .rounded(px(6.))
                        .bg(theme.bg0)
                        .font_family(ui::code_font(cx))
                        .text_size(px(11.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(preview)),
                )
            })
            .when_some(registering.error.clone(), |card, error| {
                card.child(div().text_size(px(11.5)).text_color(theme.err).child(error))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button(
                            "ssh-register-cancel",
                            i18n::t!("ssh.register_cancel"),
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.cancel_ssh_register(cx)),
                        ),
                    )
                    .child(
                        button(
                            "ssh-register-confirm",
                            i18n::t!("ssh.register_confirm"),
                            true,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.confirm_ssh_register(cx)),
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
                    cx.listener(|this, _, _window, cx| this.cancel_ssh_register(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 入れた値を config の末尾に足し、閉じて知らせる。誤りは欄を残して理由を出す（書かない）。
    #[gpui::test]
    fn a_host_is_registered_into_the_ssh_config(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_ssh_register_{}", std::process::id()));
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
        let config = root.join("ssh/config");
        let fill = |workspace: &mut Workspace, values: [&str; 5], cx: &mut Context<Workspace>| {
            let registering = workspace
                .chrome
                .ssh_registering
                .as_mut()
                .expect("開いている");
            registering.config_path = Some(config.clone());
            registering.test_after = false;
            let fields = registering.fields.clone();
            for (field, value) in fields.iter().zip(values) {
                field.update(cx, |field, cx| field.set_plain_text(value, cx));
            }
        };
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.open_ssh_register(window, cx);
            fill(workspace, ["devbox", "10.0.0.5", "me", "70000", ""], cx);
            workspace.confirm_ssh_register(cx);
            assert!(
                workspace
                    .chrome
                    .ssh_registering
                    .as_ref()
                    .is_some_and(|registering| registering.error.is_some()),
                "ポートの誤りは残して知らせる"
            );
            assert!(!config.exists(), "書かない");
            fill(workspace, ["devbox", "10.0.0.5", "me", "2222", ""], cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.focus_next_ssh_field(true, window, cx);
            workspace.confirm_ssh_register(cx);
            assert!(workspace.chrome.ssh_registering.is_none(), "閉じる");
            assert!(workspace
                .notifications
                .toasts
                .iter()
                .any(|toast| toast.text.contains("devbox")));
        });
        let written = std::fs::read_to_string(&config).expect("足した");
        assert!(
            written.contains("Host devbox\n  HostName 10.0.0.5\n  User me\n  Port 2222\n"),
            "{written}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
