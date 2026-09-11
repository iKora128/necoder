//! スマホ連携。QR 秘密をファイルやログに残さず、期限付きで設定画面に表示する。
use super::*;
use gpui::{canvas, fill, point, rgb, size, Bounds, ClipboardItem};
use std::io::Read;
use std::process::{Command, Stdio};

#[derive(Clone)]
pub(super) struct Pairing {
    url: String,
    size: usize,
    modules: Vec<u8>,
    expires: u64,
    projects: usize,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn parse_pairing(bytes: &[u8]) -> anyhow::Result<Pairing> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let url = value["url"].as_str().unwrap_or_default();
    let size = value["size"].as_u64().unwrap_or_default() as usize;
    let modules = value["modules"].as_str().unwrap_or_default().as_bytes();
    let expires = value["expires"].as_u64().unwrap_or_default();
    anyhow::ensure!(
        url.starts_with("https://") && url.len() < 4096,
        "invalid_pairing"
    );
    anyhow::ensure!(
        (21..=177).contains(&size)
            && modules.len() == size * size
            && modules.iter().all(|byte| matches!(byte, b'0' | b'1')),
        "invalid_qr"
    );
    anyhow::ensure!(
        expires > now_ms() && expires <= now_ms() + 360_000,
        "expired_pairing"
    );
    Ok(Pairing {
        url: url.to_owned(),
        size,
        modules: modules.to_vec(),
        expires,
        projects: value["tasks"].as_array().map_or(0, Vec::len),
    })
}

/// 画面に出してよい失敗コード。`relay/host`（cli.mjs / bridge.mjs）と `necoder remote`
/// が返す語をそのまま並べる。**ここに無い文字列は一切画面に出さない** — 生の stderr には
/// ペアリング URL やローカルのパスが混ざり得るため。
const KNOWN_FAILURES: &[&str] = &[
    "necoder_not_running_or_update_required",
    "open_a_project_first",
    "provision_token_missing",
    "run_init_first",
    "device_limit_revoke_unused_devices",
    "host_startup_failed",
    "local_auth_required",
    "node_missing",
    "host_not_bundled",
    "host_not_built",
    "room_create_",
    "ipc_timeout_outcome_unknown",
];

/// 子プロセスの stderr から既知の失敗コードだけ拾う（見つからなければ何も言わない）。
fn known_failure(diagnostics: &str) -> Option<&'static str> {
    KNOWN_FAILURES
        .iter()
        .find(|code| diagnostics.contains(**code))
        .copied()
}

/// 失敗コード → 画面に出す一文。未知のコードは総称の一文に倒す（原因を当て推量しない）。
fn failure_message(code: &str) -> String {
    let key = match code {
        "necoder_not_running_or_update_required" => "settings.remote_err_no_gui",
        "open_a_project_first" => "settings.remote_err_no_project",
        "provision_token_missing" | "run_init_first" => "settings.remote_err_not_initialized",
        "device_limit_revoke_unused_devices" => "settings.remote_err_device_limit",
        "node_missing" => "settings.remote_err_node",
        "host_not_bundled" | "host_not_built" => "settings.remote_err_host_missing",
        "host_startup_failed" | "local_auth_required" => "settings.remote_err_host_start",
        "pairing_timeout" | "ipc_timeout_outcome_unknown" => "settings.remote_err_timeout",
        "invalid_pairing" | "invalid_qr" | "expired_pairing" => "settings.remote_err_bad_payload",
        code if code.starts_with("room_create_") => "settings.remote_err_relay",
        _ => "settings.remote_error",
    };
    i18n::t!(key)
}

/// 失敗は**コードだけ**を返す（呼び手が [`failure_message`] で一文にする）。
fn issue_pairing() -> Result<Pairing, String> {
    run_pairing().map_err(|error| {
        // 生の stderr はログにも残さない。残すのは分類済みのコードだけ。
        eprintln!("QR を発行できない: {error}");
        error.to_string()
    })
}

fn run_pairing() -> anyhow::Result<Pairing> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["remote", "pair-json", "Phone"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing_stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing_stderr"))?;
    // Windows の小さい pipe buffer でも子の終了待ちと相互待ちにならないよう、並行して読む。
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(65_536).read_to_end(&mut bytes).map(|_| bytes)
    });
    // 失敗の理由は stderr に 1 行で来る。読み捨てずに拾って**コードだけ**を取り出す。
    let error_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(8_192).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(35);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            anyhow::bail!("pairing_timeout");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = child.wait()?;
    let output = reader
        .join()
        .map_err(|_| anyhow::anyhow!("qr_reader_failed"))??;
    let diagnostics = error_reader
        .join()
        .map_err(|_| anyhow::anyhow!("qr_reader_failed"))??;
    if !status.success() {
        // 生の stderr は持ち出さない（将来 CLI が URL を含むエラーを返しても漏らさない）。
        // 既知のコードに当たらなければ総称の失敗＝画面では「原因不明」と正直に言う。
        let code =
            known_failure(&String::from_utf8_lossy(&diagnostics)).unwrap_or("pairing_failed");
        anyhow::bail!("{code}");
    }
    parse_pairing(&output)
}

impl SettingsView {
    /// 隔離した UI 撮影用。公開鍵やユーザー設定を変更せず、ダミー QR だけを描画する。
    #[cfg(feature = "remote-preview")]
    pub fn remote_preview(&mut self, payload: &[u8]) {
        self.remote_pairing = parse_pairing(payload).ok();
        self.remote_preview_only = true;
    }
    fn issue_remote_qr(&mut self, cx: &mut Context<Self>) {
        if self.remote_busy {
            return;
        }
        self.remote_busy = true;
        self.remote_pairing = None;
        self.remote_error = None;
        self.remote_generation += 1;
        let generation = self.remote_generation;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async { issue_pairing() })
                .await;
            let expires = result.as_ref().ok().map(|pairing| pairing.expires);
            if view
                .update(cx, |view, cx| {
                    view.remote_busy = false;
                    match result {
                        Ok(pairing) => view.remote_pairing = Some(pairing),
                        Err(code) => view.remote_error = Some(failure_message(&code).into()),
                    }
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            if let Some(expires) = expires {
                cx.background_executor()
                    .timer(Duration::from_millis(expires.saturating_sub(now_ms())))
                    .await;
                if let Err(error) = view.update(cx, |view, cx| {
                    if view.remote_generation == generation {
                        view.remote_pairing = None;
                        view.remote_error = Some(i18n::t!("settings.remote_expired").into());
                        cx.notify();
                    }
                }) {
                    eprintln!("Remote QR view closed: {error}");
                }
            }
        })
        .detach();
    }

    pub(super) fn remote_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = &self.theme;
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(8.))
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(12.))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3))
                .child(SharedString::from(label))
        };
        let control = if self.remote_busy {
            div()
                .child(SharedString::from(i18n::t!("settings.cli_busy")))
                .into_any_element()
        } else {
            button("remote-issue-qr", i18n::t!("settings.remote_issue"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, _, _, cx| view.issue_remote_qr(cx)),
                )
                .into_any_element()
        };
        div().flex().flex_col().gap(px(8.))
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.fg1)
                .child(SharedString::from(i18n::t!("settings.remote_heading"))))
            .child(self.pref_row(i18n::t!("settings.remote_label"), Some(i18n::t!("settings.remote_sub")), control))
            .when_some(self.remote_error.clone(), |element, error| element.child(div().text_size(px(12.)).text_color(theme.fg2).child(error)))
            .when_some(self.remote_pairing.clone().filter(|pairing| pairing.expires > now_ms()), |element, pairing| {
                let matrix = pairing.modules;
                let side = pairing.size;
                // 4 module の quiet zone を確保し、テーマに関係なく白地に黒で描画。
                let extent = ((side + 8) * 4) as f32;
                element.child(div().flex().flex_col().gap(px(8.))
                    .child(canvas(|_, _, _| (), move |bounds, _, window, _| {
                        window.paint_quad(fill(bounds, rgb(0xffffff)));
                        for (index, byte) in matrix.iter().enumerate() {
                            if *byte == b'1' {
                                let origin = bounds.origin + point(px(((index % side + 4) * 4) as f32), px(((index / side + 4) * 4) as f32));
                                window.paint_quad(fill(Bounds::new(origin, size(px(4.), px(4.))), rgb(0)));
                            }
                        }
                    }).w(px(extent)).h(px(extent)))
                    .child(div().text_size(px(12.)).text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("settings.remote_qr_hint", "count" => pairing.projects))))
                    .child(button("remote-copy-url", i18n::t!("settings.remote_copy"))
                        .on_mouse_down(MouseButton::Left, cx.listener(|view, _, _, cx| {
                            if let Some(pairing) = view.remote_pairing.as_ref().filter(|pairing| pairing.expires > now_ms()) {
                                cx.write_to_clipboard(ClipboardItem::new_string(pairing.url.clone()));
                            }
                        })))
                    .child(button("remote-hide-qr", i18n::t!("settings.remote_hide"))
                        .on_mouse_down(MouseButton::Left, cx.listener(|view, _, _, cx| {
                            view.remote_pairing = None; view.remote_generation += 1; cx.notify();
                        }))))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qr_payload_validation() {
        let mut value = serde_json::json!({"url":"https://control.necoder.com/#test", "size":21,
            "modules":"0".repeat(441), "expires":now_ms()+300000, "tasks":["a","b"]});
        assert_eq!(
            parse_pairing(value.to_string().as_bytes())
                .unwrap()
                .projects,
            2
        );
        value["modules"] = "bad".into();
        assert!(parse_pairing(value.to_string().as_bytes()).is_err());
        value["modules"] = "0".repeat(441).into();
        value["expires"] = 0.into();
        assert!(parse_pairing(value.to_string().as_bytes()).is_err());
    }

    /// stderr から拾うのは**既知のコードだけ**。URL やパスの混ざった行からは何も取らない。
    #[test]
    fn only_known_codes_are_picked_from_stderr() {
        assert_eq!(
            known_failure("necoder_not_running_or_update_required\n"),
            Some("necoder_not_running_or_update_required")
        );
        assert_eq!(known_failure("room_create_503"), Some("room_create_"));
        assert_eq!(
            known_failure("Error: https://control.necoder.com/#room=secret"),
            None
        );
    }

    /// 原因ごとに違う一文を出す（「Node.js 22 以降」を全部の失敗に貼らない）。
    #[test]
    fn failures_map_to_their_own_message() {
        let generic = failure_message("pairing_failed");
        let no_gui = failure_message("necoder_not_running_or_update_required");
        assert_ne!(no_gui, generic);
        assert_ne!(failure_message("open_a_project_first"), generic);
        assert_ne!(failure_message("room_create_503"), generic);
        assert_eq!(failure_message("なにか未知の失敗"), generic);
        // 翻訳漏れならキー文字列がそのまま返る（i18n::translate の仕様）。
        assert!(
            !no_gui.starts_with("settings."),
            "ja/en に対訳が無い: {no_gui}"
        );
    }
}
