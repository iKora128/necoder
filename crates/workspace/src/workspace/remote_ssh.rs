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

    /// リモート SSH ホストピッカー（M13）: SSH config の Host 一覧 + 末尾に「手入力」。
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
                    let server_command = std::env::var("NECODER_REMOTE_SERVER_COMMAND")
                        .unwrap_or_else(|_| "necoder-remote-server".to_string());
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

// ── GUI askpass（ROADMAP「残: GUI askpass」） ──
// GUI から起こす ssh には TTY が無い。OpenSSH は `SSH_ASKPASS` があればそちらに訊くので、
// necoder 自身を askpass に指して（host::install_askpass）ここへ中継する。鍵・agent・
// ProxyJump は今までどおり system ssh のままで、「鍵が無いホストに入れない」だけを埋める。
//
// 秘密は **画面と ssh の間にしか置かない**: 設定にも DB にも履歴にも書かない
// （research/remote-ssh-2026.md「password は設定へ永続化しない」）。

/// askpass の入力欄 1 回ぶん。`respond` は要求元（askpass として起動した necoder）へ戻す口で、
/// Enter で秘密、Escape で取り消しを返す。**Drop だけは必ず応答する**（下の `Drop` 実装）——
/// 返さないまま消すと ssh が askpass の終了を待ち続ける。
pub(crate) struct AskpassPrompt {
    /// OpenSSH が渡してきた原文（"user@host's password:" など）。そのまま見せる。
    pub(crate) prompt: SharedString,
    /// 同じ問いの何回目か（1 = 初回）。2 以上なら直前の入力が拒否されている。
    pub(crate) attempt: u32,
    /// 入力中の秘密。画面には伏せ字でしか出さない。
    pub(crate) value: String,
    pub(crate) focus: FocusHandle,
    /// `None` = 応答済み（二重応答を防ぐ）。
    pub(crate) respond: Option<std::sync::mpsc::Sender<serde_json::Value>>,
    /// キャレットの点灯（`askpass_caret_ticker` が 530ms ごとに反転）。
    pub(crate) caret_on: bool,
    /// 点滅タイマーが回っているか（二重起動を防ぐ）。
    pub(crate) blinking: bool,
}

impl AskpassPrompt {
    /// 秘密（または取り消し）を要求元へ返す。以後この prompt は応答済みになる。
    fn answer(&mut self, response: serde_json::Value) {
        if let Some(respond) = self.respond.take() {
            if respond.send(response).is_err() {
                // 要求元が既に消えている（ssh が諦めた）。捨てる以外にできることは無い。
            }
        }
    }
}

impl Drop for AskpassPrompt {
    fn drop(&mut self) {
        // 窓ごと閉じられた場合でも「取り消し」を返す。無言で消えると ssh 側が待ち続ける。
        self.answer(control_ipc::err(i18n::t!("askpass.cancelled")));
    }
}

impl Workspace {
    /// askpass 要求を受けて入力欄を開く（`control_ipc` の `askpass` メソッドから）。
    /// 既に開いていれば、新しい要求は開かずに取り消しを返す（同時に 2 つは出さない）。
    pub(crate) fn open_askpass(
        &mut self,
        prompt: String,
        attempt: u32,
        respond: std::sync::mpsc::Sender<serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        if self.overlays.askpass.is_some() {
            if respond
                .send(control_ipc::err(i18n::t!("askpass.err_busy")))
                .is_err()
            {
                // 要求元が消えているだけ。
            }
            return;
        }
        let focus = cx.focus_handle();
        self.overlays.askpass = Some(AskpassPrompt {
            prompt: SharedString::from(prompt),
            attempt,
            value: String::new(),
            focus: focus.clone(),
            respond: Some(respond),
            caret_on: true,
            blinking: false,
        });
        // フォーカスは**入力欄が 1 度描画された後**に当てる。まだ dispatch tree に居ない handle を
        // focus しても、次フレームで「focus 先が居ない」と判定され、focus-lost 復帰
        // （`Workspace::render` の `on_focus_lost`）が直前の可視面へ引き戻す = 入力欄は出ているのに
        // 打鍵がエディタへ流れる。描画を挟むため、次の effect cycle（`process_pending_shell_effects`）
        // へ回す。
        self.chrome.pending_askpass_focus = true;
        // 接続待ちの裏で来るので、窓が後ろにいると入力できないことに気づけない。
        cx.activate(true);
        cx.notify();
    }

    /// キャレット点滅。入力欄が開いていてフォーカスがある間だけ回り、それ以外では自分で止まる
    /// （`ensure_visual_ticker` と同じ流儀）。打てる状態が一目で分かることが目的。
    pub(crate) fn ensure_askpass_caret_blink(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.overlays.askpass.as_mut() else {
            return;
        };
        if prompt.blinking {
            return;
        }
        prompt.blinking = true;
        cx.spawn(async move |workspace, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(530))
                .await;
            let keep_going = workspace.update(cx, |workspace, cx| {
                let Some(prompt) = workspace.overlays.askpass.as_mut() else {
                    return false;
                };
                prompt.caret_on = !prompt.caret_on;
                cx.notify();
                true
            });
            if !matches!(keep_going, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// 入力欄を閉じる（`answer` 済みなら何も返さない・未応答なら `Drop` が取り消しを返す）。
    fn close_askpass(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlays.askpass.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn on_askpass_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_askpass(window, cx),
            "enter" => {
                if let Some(prompt) = self.overlays.askpass.as_mut() {
                    let secret = std::mem::take(&mut prompt.value);
                    prompt.answer(control_ipc::ok(
                        serde_json::json!({ "secret": secret }),
                    ));
                }
                self.close_askpass(window, cx);
            }
            "backspace" => {
                if let Some(prompt) = self.overlays.askpass.as_mut() {
                    prompt.value.pop();
                    prompt.caret_on = true;
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
                if let Some(prompt) = self.overlays.askpass.as_mut() {
                    prompt.value.push_str(text);
                    prompt.caret_on = true;
                    cx.notify();
                }
            }
        }
    }

    /// 伏せ字の入力欄。文字数だけが見える（VSCode / ターミナルの password 入力と同じ約束）。
    pub(crate) fn render_askpass(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let prompt = self.overlays.askpass.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let masked: SharedString = SharedString::from("•".repeat(prompt.value.chars().count()));
        // 打てる状態かどうかを見た目で分ける。ここが分からないと「出ているのに打てない」
        // （裏のエディタへ流れている）状態に気づけない。
        let focused = prompt.focus.is_focused(window);
        let border = if focused { accent } else { theme.border };
        let hint = if focused {
            i18n::t!("askpass.hint")
        } else {
            i18n::t!("askpass.hint_click")
        };
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
                        .flex_col()
                        .gap(px(6.))
                        .p(px(12.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(border)
                        .rounded(px(8.))
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(6.),
                            gpui::hsla(0., 0., 0., 0.4),
                        )
                        .blur_radius(px(16.))])
                        .track_focus(&prompt.focus)
                        .on_key_down(cx.listener(Self::on_askpass_key_down))
                        // 入力欄の上のクリックを裏のエディタへ通さない。通すとエディタが
                        // フォーカスを奪い、入力欄は出たまま打鍵だけがエディタへ流れる。
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|workspace, _event, window, cx| {
                                if let Some(prompt) = workspace.overlays.askpass.as_ref() {
                                    let focus = prompt.focus.clone();
                                    window.focus(&focus, cx);
                                }
                            }),
                        )
                        .text_size(px(12.5))
                        .text_color(theme.fg0)
                        .children((prompt.attempt > 1).then(|| {
                            // 入力を間違えると OpenSSH は同じ問いで訊き直す。黙って同じ欄が
                            // 出るだけだと「打てなかったのか / 間違えたのか」が分からない。
                            div()
                                .text_size(px(11.))
                                .text_color(theme.warn)
                                .child(SharedString::from(i18n::t!(
                                    "askpass.retry",
                                    "attempt" => prompt.attempt
                                )))
                        }))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(prompt.prompt.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(24.))
                                // 伏せ字 → キャレット → 余白の順。キャレットを `flex_1` の
                                // 後ろに置くと、文字数に関係なく右端へ張り付いてしまう。
                                .child(
                                    div()
                                        .flex_none()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(masked),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(1.5))
                                        .h(px(14.))
                                        .bg(if focused && prompt.caret_on {
                                            accent
                                        } else {
                                            theme.bg2
                                        }),
                                )
                                .child(div().flex_1()),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from(hint)),
                        ),
                )
                .into_any_element(),
        )
    }
}
