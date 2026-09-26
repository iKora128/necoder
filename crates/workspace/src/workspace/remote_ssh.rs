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
        self.picker_ssh_testing = false;
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

    /// パレット「SSH: 接続を確かめる…」（O37・G01）: 同じホストピッカーを「試すだけ」で開く。
    pub(crate) fn open_ssh_test_picker(
        &mut self,
        _: &TestSshConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_ssh_host_picker(&RemoteSsh, window, cx);
        self.picker_ssh_testing = true;
    }

    /// `uri` の接続を確かめる（開かない）。繋がれば掛かった時間、だめなら理由と次にすること
    /// （[`Self::report_ssh_failure`]・接続に失敗した時と同じ案内）。
    pub(crate) fn test_ssh_uri(&mut self, uri: String, cx: &mut Context<Self>) {
        let project = match host::SshProject::parse(&uri) {
            Ok(project) => project,
            Err(error) => {
                self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx);
                return;
            }
        };
        let host_name = project.host.clone();
        self.push_toast(
            SharedString::from(i18n::t!("ssh.testing", "host" => host_name.clone())),
            self.accent(),
            cx,
        );
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { host::test_ssh_connection(&project) })
                .await;
            let finished = workspace.update(cx, |workspace, cx| match result {
                Ok(elapsed) => workspace.push_toast(
                    SharedString::from(i18n::t!(
                        "ssh.test_ok",
                        "host" => host_name.clone(),
                        "ms" => elapsed.as_millis()
                    )),
                    workspace.accent(),
                    cx,
                ),
                Err(error) => workspace.report_ssh_failure(&host_name, &error, cx),
            });
            if let Err(error) = finished {
                eprintln!("SSH の接続テストの結果を出せない: {error:#}");
            }
        })
        .detach();
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
                    Err(error) => {
                        let host_name = last_path
                            .as_ref()
                            .map(|(host_name, _)| host_name.as_str())
                            .unwrap_or_default();
                        workspace.report_ssh_failure(host_name, &error, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 接続に失敗した時の知らせ（O37）。OpenSSH の理由が分かれば、何をすればよいかを出す
    /// （指紋・鍵と ssh-agent / Keychain・名前・拒否・応答なし…）。OpenSSH が書いた全文は
    /// 「全文 ›」で読める。分からなければ今までどおり理由をそのまま出す。
    pub(crate) fn report_ssh_failure(
        &mut self,
        host_name: &str,
        error: &anyhow::Error,
        cx: &mut Context<Self>,
    ) {
        let failure = error
            .downcast_ref::<host::SshConnectError>()
            .and_then(|connect| connect.failure);
        let Some(failure) = failure else {
            self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx);
            return;
        };
        let text = format!(
            "{}\n{}",
            i18n::t!("ssh.connect_failed", "host" => host_name),
            ssh_failure_hint(failure, host_name)
        );
        self.push_failure_toast(
            SharedString::from(text),
            Some((
                SharedString::from(i18n::t!("ssh.failure_details")),
                format!("{error:#}"),
            )),
            cx,
        );
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

/// OpenSSH の失敗の種類ごとの「次にすること」（O37・G02 / G04）。
pub(crate) fn ssh_failure_hint(failure: host::SshFailure, host_name: &str) -> String {
    use host::SshFailure;
    let key = match failure {
        SshFailure::HostKeyUnknown => "ssh.hint_host_key_unknown",
        SshFailure::HostKeyChanged => "ssh.hint_host_key_changed",
        SshFailure::Authentication => "ssh.hint_authentication",
        SshFailure::TooManyKeys => "ssh.hint_too_many_keys",
        SshFailure::UnknownHost => "ssh.hint_unknown_host",
        SshFailure::Refused => "ssh.hint_refused",
        SshFailure::TimedOut => "ssh.hint_timed_out",
        SshFailure::BadPermissions => "ssh.hint_bad_permissions",
        SshFailure::Negotiation => "ssh.hint_negotiation",
    };
    i18n::t!(key, "host" => host_name)
}

/// 鍵のパスフレーズを訊かれた時の「毎回訊かれないために」の一言（O37・G04）。OpenSSH の問いは
/// `Enter passphrase for key '/Users/me/.ssh/id_ed25519': ` の形なので、その鍵で `ssh-add` の
/// コマンドを組んで見せる（macOS は Keychain にも預ける `--apple-use-keychain`）。パスワードなど
/// ほかの問いには出さない。ssh は手元で動くので、分岐は手元の OS でよい。
pub(crate) fn passphrase_tip(prompt: &str) -> Option<String> {
    let rest = prompt.split("passphrase for key").nth(1)?;
    let key = rest.split('\'').nth(1).filter(|key| !key.is_empty())?;
    let key = if key.contains(char::is_whitespace) {
        format!("'{key}'")
    } else {
        key.to_string()
    };
    let command = if cfg!(target_os = "macos") {
        format!("ssh-add --apple-use-keychain {key}")
    } else {
        format!("ssh-add {key}")
    };
    Some(i18n::t!("askpass.passphrase_tip", "command" => command))
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
        // パスワード欄の文字や Enter を背後のキー処理へ流さない。
        cx.stop_propagation();
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
                        )
                        .children(passphrase_tip(&prompt.prompt).map(|tip| {
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from(tip))
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::notifications::ToastAction;
    use host::{SshConnectError, SshFailure};

    #[test]
    fn every_openssh_failure_has_its_own_hint() {
        let failures = [
            SshFailure::HostKeyUnknown,
            SshFailure::HostKeyChanged,
            SshFailure::Authentication,
            SshFailure::TooManyKeys,
            SshFailure::UnknownHost,
            SshFailure::Refused,
            SshFailure::TimedOut,
            SshFailure::BadPermissions,
            SshFailure::Negotiation,
        ];
        let hints: Vec<String> = failures
            .iter()
            .map(|failure| ssh_failure_hint(*failure, "devbox"))
            .collect();
        for hint in &hints {
            assert!(!hint.starts_with("ssh."), "訳が無い: {hint}");
        }
        let mut distinct = hints.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), hints.len(), "種類ごとに違う案内");
        assert!(
            ssh_failure_hint(SshFailure::HostKeyChanged, "devbox").contains("ssh-keygen -R devbox")
        );
    }

    #[test]
    fn a_key_passphrase_prompt_offers_ssh_add() {
        let tip = passphrase_tip("Enter passphrase for key '/Users/me/.ssh/id_ed25519': ")
            .expect("鍵のパスフレーズ");
        assert!(tip.contains("ssh-add"), "{tip}");
        assert!(tip.contains("/Users/me/.ssh/id_ed25519"), "{tip}");
        let spaced = passphrase_tip("Enter passphrase for key '/Users/me/my keys/id': ").unwrap();
        assert!(
            spaced.contains("'/Users/me/my keys/id'"),
            "空白を含むパスは囲む: {spaced}"
        );
        assert_eq!(passphrase_tip("me@devbox's password: "), None);
        assert_eq!(passphrase_tip("Enter passphrase for key '': "), None);
    }

    #[gpui::test]
    fn the_test_picker_only_tests_and_the_normal_picker_opens(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_ssh_test_picker_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let folders = vec![root.clone()];
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(folders, Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.open_ssh_test_picker(&TestSshConnection, window, cx);
            assert!(workspace.picker_ssh_testing);
            assert!(workspace.overlays.picker_mode == PickerMode::SshHosts);
            // 末尾の「手入力」を選んでも、試すだけの時は入力バーを開かない。
            let manual = workspace.picker_ssh_recent.len() + workspace.picker_ssh_hosts.len();
            let picker = workspace.overlays.picker.clone().expect("ピッカー");
            workspace.on_picker_event(&picker, &PickerEvent::Confirmed(manual), window, cx);
            assert!(!workspace.picker_ssh_testing, "1 回で外れる");
            assert!(workspace.overlays.ssh_input.is_none(), "開かない");

            // 普通に開けば試す印は付かない（手入力は入力バーを開く）。
            workspace.open_ssh_host_picker(&RemoteSsh, window, cx);
            assert!(!workspace.picker_ssh_testing);
            let manual = workspace.picker_ssh_recent.len() + workspace.picker_ssh_hosts.len();
            let picker = workspace.overlays.picker.clone().expect("ピッカー");
            workspace.on_picker_event(&picker, &PickerEvent::Confirmed(manual), window, cx);
            assert!(workspace.overlays.ssh_input.is_some(), "手入力は入力バー");
        });
        workspace.update(cx, |workspace, _cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui::test]
    fn a_rejected_key_is_explained_and_the_log_kept(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!("necoder_ssh_failure_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let folders = vec![root.clone()];
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(folders, Theme::dark(), None, cx));
        workspace.update(cx, |workspace, cx| {
            let rejected = anyhow::Error::new(SshConnectError::new(
                "exit status: 255",
                "me@devbox: Permission denied (publickey).\n",
            ))
            .context("SSH ControlMaster の確立に失敗");
            workspace.report_ssh_failure("devbox", &rejected, cx);
            let toast = workspace.notifications.toasts.last().expect("知らせ");
            assert!(
                toast
                    .text
                    .starts_with(&i18n::t!("ssh.connect_failed", "host" => "devbox")),
                "{}",
                toast.text
            );
            assert!(
                toast
                    .text
                    .contains(&ssh_failure_hint(SshFailure::Authentication, "devbox")),
                "鍵の案内: {}",
                toast.text
            );
            match &toast.action {
                Some(ToastAction::OpenDetails { text, .. }) => {
                    assert!(text.contains("Permission denied (publickey)"), "{text}")
                }
                _ => panic!("全文を読める"),
            }

            let unknown = anyhow::anyhow!("remote-server の配備に失敗: sh: 1: not found");
            workspace.report_ssh_failure("devbox", &unknown, cx);
            let toast = workspace.notifications.toasts.last().expect("知らせ");
            assert_eq!(
                toast.text.as_ref(),
                "remote-server の配備に失敗: sh: 1: not found",
                "分からない失敗は今までどおり"
            );
        });
        workspace.update(cx, |workspace, _cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
