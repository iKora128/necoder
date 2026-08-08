use crate::workspace::*;

// ── リモート SSH の GUI 導線（M13） ──
// ssh:// 入力バー・~/.ssh/config ホストピッカー・接続と履歴記録。接続は system OpenSSH に
// 委ねる（鍵・ProxyJump・agent は config のまま効く）。

impl Workspace {
    /// titlebar の SSH ボタン: `ssh://user@host/path` の入力バーを開く（goto/rename と同型）。
    /// seed は「アクティブが remote ならその URI・そうでなければ ssh://」。
    #[cfg(debug_assertions)]
    pub(crate) fn open_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self
            .active_slot()
            .and_then(|slot| {
                let host = slot.worktree.host();
                host.is_remote()
                    .then(|| format!("{}{}", host.display_name(), slot.worktree.root().display()))
            })
            .map(|identity| {
                // display_name は "SSH user@host" 形式 → ssh://user@host に直す。
                identity.replace("SSH ", "ssh://").replace(" ", "")
            })
            .unwrap_or_else(|| "ssh://".to_string());
        self.open_ssh_input_seeded(seed, window, cx);
    }

    /// SSH 入力バーを種文字列付きで開く（ホストピッカーからの遷移でも使う）。
    pub(crate) fn open_ssh_input_seeded(
        &mut self,
        seed: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlays.ssh_connecting {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.overlays.ssh_input = Some((seed, focus));
        cx.notify();
    }

    /// リモート SSH ホストピッカー（M13）: `~/.ssh/config` の Host 一覧 + 末尾に「手入力」。
    /// 選択で `ssh://<alias>/` を種に入力バーへ（パスだけ足して Enter で接続）。
    /// system OpenSSH に委ねるので User/HostName/鍵/ProxyJump は config のものがそのまま効く。
    pub(crate) fn open_ssh_host_picker(
        &mut self,
        _: &RemoteSsh,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hosts = host::ssh_config_hosts();
        // 2階層: 上=最近のリモートプロジェクト（履歴・直接接続・#5）、下=config ホスト、末尾=手入力。
        let recent = self
            .persistence
            .storage
            .as_ref()
            .and_then(|storage| storage.recent_remote_projects().ok())
            .unwrap_or_default();
        let mut items: Vec<PickerItem> = Vec::new();
        let mut recent_uris: Vec<String> = Vec::new();
        for (host_key, path, name, _opened_at) in recent.iter().take(20) {
            let id = recent_uris.len();
            items.push(
                PickerItem::new(id, name.clone())
                    .with_accent(self.accent()) // 行頭●= 最近のプロジェクトの目印
                    .with_detail(format!("{host_key}:{path}")),
            );
            recent_uris.push(format!("ssh://{host_key}{path}"));
        }
        let recent_count = recent_uris.len();
        // config ホスト（id = recent_count + index。選ぶと前回パス即接続 or パス入力）。
        for (offset, host) in hosts.iter().enumerate() {
            let mut item = PickerItem::new(recent_count + offset, host.alias.clone());
            let base = match (&host.user, &host.hostname) {
                (Some(user), Some(hostname)) => Some(format!("{user}@{hostname}")),
                (None, Some(hostname)) => Some(hostname.clone()),
                (Some(user), None) => Some(format!("{user}@{}", host.alias)),
                (None, None) => None,
            };
            // 前回パスがあれば併記（→ が即接続先・#2d）。
            let last_path = self
                .persistence
                .storage
                .as_ref()
                .and_then(|storage| storage.host_last_path(&host.alias).ok().flatten());
            let detail = match (base, last_path) {
                (Some(base), Some(path)) => Some(format!("{base}  →{path}")),
                (Some(base), None) => Some(base),
                (None, Some(path)) => Some(format!("→{path}")),
                (None, None) => None,
            };
            if let Some(detail) = detail {
                item = item.with_detail(detail);
            }
            items.push(item);
        }
        // 末尾は「手入力」= 生の ssh:// 入力バー（config に無いホストへの逃げ道）。
        items.push(
            PickerItem::new(recent_count + hosts.len(), i18n::t!("ssh.manual_entry"))
                .with_detail("ssh://user@host/path"),
        );
        self.picker_ssh_recent = recent_uris;
        self.picker_ssh_hosts = hosts;
        self.open_picker(
            PickerMode::SshHosts,
            i18n::t!("ssh.picker_placeholder"),
            items,
            window,
            cx,
        );
    }

    pub(crate) fn close_ssh_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlays.ssh_input.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn on_ssh_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_ssh_input(window, cx),
            "enter" => {
                let uri = self
                    .overlays
                    .ssh_input
                    .as_ref()
                    .map(|(value, _)| value.trim().to_string())
                    .unwrap_or_default();
                self.close_ssh_input(window, cx);
                if !uri.is_empty() && uri != "ssh://" {
                    self.connect_ssh_and_open(uri, cx);
                }
            }
            "backspace" => {
                if let Some((value, _)) = self.overlays.ssh_input.as_mut() {
                    value.pop();
                    cx.notify();
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
                if let Some((value, _)) = self.overlays.ssh_input.as_mut() {
                    value.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    /// SSH 接続 →（成功したら）**現在のウィンドウのレールに開く**（ウィンドウモデル: 新窓は
    /// 明示操作のときだけ・ユーザー要件「勝手に窓を開かない」）。接続は ControlMaster + server 配備で
    /// 数秒〜かかるため背景で行い、失敗はトーストで返す。system OpenSSH に委ねるので
    /// ~/.ssh/config の Host エイリアス・鍵・ProxyJump・agent がそのまま効く。
    pub(crate) fn connect_ssh_and_open(&mut self, uri: String, cx: &mut Context<Self>) {
        self.overlays.ssh_connecting = true;
        self.push_toast(
            SharedString::from(i18n::t!("ssh.connecting", "uri" => uri.clone())),
            self.accent(),
            cx,
        );
        cx.notify();
        // 成功したらホスト別に前回パスを記録（#2d・次回は打たずに即接続）。
        let last_path = host::SshProject::parse(&uri)
            .ok()
            .map(|project| (project.host, project.path.to_string_lossy().to_string()));
        let storage = self.persistence.storage.clone();
        cx.spawn(async move |workspace, cx| {
            let source = cx
                .background_executor()
                .spawn(async move {
                    let project = host::SshProject::parse(&uri)?;
                    let server_command = std::env::var("SHIRUSHI_REMOTE_SERVER_COMMAND")
                        .unwrap_or_else(|_| "shirushi-remote-server".to_string());
                    let remote = host::RemoteHost::connect_ssh(&project, &server_command)?;
                    let root = remote.root().to_path_buf();
                    Ok::<ProjectSource, anyhow::Error>(ProjectSource::new(remote, root))
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.overlays.ssh_connecting = false;
                match source {
                    Ok(source) => {
                        // path 未指定（home）= ブラウズ入口。具体パス = 開いたプロジェクト。
                        let browse = last_path
                            .as_ref()
                            .map(|(_, path)| path.is_empty() || path == "/")
                            .unwrap_or(true);
                        if let (Some(storage), Some((host_key, path))) = (&storage, &last_path) {
                            // home はブラウズ入口なので履歴/前回パスに残さない（#5・home≠プロジェクト）。
                            if !browse {
                                let _ = storage.set_host_last_path(host_key, path);
                                let name = std::path::Path::new(path)
                                    .file_name()
                                    .map(|component| component.to_string_lossy().to_string())
                                    .filter(|component| !component.is_empty())
                                    .unwrap_or_else(|| host_key.clone());
                                let _ = storage.record_remote_project(host_key, path, &name);
                            }
                        }
                        // 新窓ではなく現在のウィンドウのレールに開く（remote host 接続を再利用・
                        // 再接続なし）。新窓が要るときは explorer 右クリック「新しいウィンドウで開く」で明示。
                        workspace.open_folder_in_rail(
                            source.host().clone(),
                            source.root().to_path_buf(),
                            None,
                            cx,
                        );
                        // home に繋いだ = ブラウズ。ツリーを辿って右クリック→プロジェクト化、を促す
                        // （「cd 連打→開き直し」を消す browse-first 導線の入口）。
                        if browse {
                            workspace.push_toast(
                                SharedString::from(i18n::t!("ssh.browse_hint")),
                                workspace.accent(),
                                cx,
                            );
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

    /// SSH 入力バー（rename/goto と同型の中央上オーバーレイ）。
    pub(crate) fn render_ssh_input(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (value, focus) = self.overlays.ssh_input.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let display: SharedString = SharedString::from(value.clone());
        Some(
            div()
                .absolute()
                .top(px(96.))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(460.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(30.))
                        .px(px(10.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(accent)
                        .rounded(px(8.))
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(6.),
                            gpui::hsla(0., 0., 0., 0.4),
                        )
                        .blur_radius(px(16.))])
                        .track_focus(focus)
                        .on_key_down(cx.listener(Self::on_ssh_key_down))
                        .text_size(px(12.5))
                        .text_color(theme.fg0)
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("ssh.label"))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(display),
                        )
                        .child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent)),
                )
                .into_any_element(),
        )
    }
}
