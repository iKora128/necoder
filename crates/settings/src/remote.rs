//! スマホ連携。QR 秘密をファイルやログに残さず、期限付きで設定画面に表示する。
//! 登録済みの端末を一覧し、その場で削除（失効）できる — 離席中に誰かが QR を発行して
//! 自分の端末を繋いでも、ここを開けば気付けて切れるように。
use super::*;
use gpui::{canvas, fill, point, rgb, size, Bounds, ClipboardItem};
use std::io::Read;
use std::process::Stdio;
use std::time::Instant;

/// 「登録済みの端末」を読み直す間隔（ページが表示されている間だけ）。
const DEVICE_REFRESH: Duration = Duration::from_secs(5);
/// ホストが受け付ける端末の種類（`relay/host/bridge.mjs` の `DEVICE_MODELS` と同じ）。
const DEVICE_MODELS: &[&str] = &["iPhone", "iPad", "Android", "Mac", "Windows", "Linux"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DeviceStatus {
    /// 認証済みの経路が開いている。
    Online,
    /// 登録済みだが、いまは繋がっていない（ホストが止まっている時も含む）。
    Offline,
    /// QR を発行したが、まだ読み取られていない。
    Waiting,
}

#[derive(Clone, Debug)]
pub(super) struct RemoteDevice {
    id: String,
    model: Option<String>,
    status: DeviceStatus,
    paired_at: Option<u64>,
    last_seen: Option<u64>,
    projects: usize,
}

/// 設定画面の「登録済みの端末」の状態。
#[derive(Default)]
pub(super) struct DeviceList {
    /// `None` = まだ一度も読めていない。
    devices: Option<Vec<RemoteDevice>>,
    error: Option<SharedString>,
    loading: bool,
    loaded_at: Option<Instant>,
    /// 「削除」を押して確認を待っている端末。
    confirming: Option<String>,
    /// 削除を実行中の端末。
    removing: Option<String>,
}

#[derive(Clone)]
pub(super) struct Pairing {
    url: String,
    size: usize,
    modules: Vec<u8>,
    expires: u64,
    projects: usize,
}

impl DeviceList {
    /// 次の描画で読み直させる。
    pub(super) fn mark_stale(&mut self) {
        self.loaded_at = None;
    }
}

/// ホストの端末 ID（部屋 ID・base64url 43 文字）。CLI の引数に渡すので形を確かめてから使う。
fn valid_device_id(id: &str) -> bool {
    id.len() == 43
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn parse_devices(bytes: &[u8]) -> anyhow::Result<Vec<RemoteDevice>> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let devices = value["devices"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("invalid_devices"))?;
    devices
        .iter()
        .map(|device| {
            let id = device["id"]
                .as_str()
                .filter(|id| valid_device_id(id))
                .ok_or_else(|| anyhow::anyhow!("invalid_devices"))?;
            let status = match device["status"].as_str() {
                Some("online") => DeviceStatus::Online,
                Some("offline") => DeviceStatus::Offline,
                Some("waiting") => DeviceStatus::Waiting,
                _ => anyhow::bail!("invalid_devices"),
            };
            Ok(RemoteDevice {
                id: id.to_owned(),
                model: device["model"]
                    .as_str()
                    .filter(|model| DEVICE_MODELS.contains(model))
                    .map(str::to_owned),
                status,
                paired_at: device["paired_at"].as_u64(),
                last_seen: device["last_seen"].as_u64(),
                projects: device["projects"].as_array().map_or(0, Vec::len),
            })
        })
        .collect()
}

/// unix ms を「たった今 / N分前 / N時間前 / N日前」へ（タイムゾーンの依存を増やさない）。
fn relative_time(unix_ms: u64) -> String {
    let minutes = now_ms().saturating_sub(unix_ms) / 60_000;
    if minutes < 1 {
        i18n::t!("time.just_now")
    } else if minutes < 60 {
        i18n::t!("time.minutes_ago", "count" => minutes)
    } else if minutes < 60 * 24 {
        i18n::t!("time.hours_ago", "count" => minutes / 60)
    } else {
        i18n::t!("time.days_ago", "count" => minutes / (60 * 24))
    }
}

/// 端末の 2 行目（登録・最終接続・共有しているプロジェクト数）。
fn device_details(device: &RemoteDevice) -> String {
    let mut parts = Vec::new();
    if device.status != DeviceStatus::Waiting {
        if let Some(paired_at) = device.paired_at {
            parts.push(
                i18n::t!("settings.remote_device_paired", "when" => relative_time(paired_at)),
            );
        }
        // 接続中なら「最終接続」は言うまでもない。
        if device.status == DeviceStatus::Offline {
            if let Some(last_seen) = device.last_seen {
                parts.push(
                    i18n::t!("settings.remote_device_seen", "when" => relative_time(last_seen)),
                );
            }
        }
    }
    parts.push(i18n::t!("settings.remote_device_projects", "count" => device.projects));
    parts.join(" · ")
}

/// ペアリング URL のフラグメントにある部屋 ID（= 端末 ID）。
fn pairing_room(url: &str) -> Option<&str> {
    url.split_once('#')?
        .1
        .split('&')
        .find_map(|pair| pair.strip_prefix("room="))
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
    "run_init_first",
    "device_limit_revoke_unused_devices",
    "host_startup_failed",
    "local_auth_required",
    "node_missing",
    "host_not_bundled",
    "host_not_built",
    // 429 は "room_create_" より前に置く（known_failure は先に一致した要素を返す）。
    "room_create_429",
    "room_create_",
    "proof_of_work_too_hard",
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
        "run_init_first" => "settings.remote_err_not_initialized",
        "device_limit_revoke_unused_devices" => "settings.remote_err_device_limit",
        "node_missing" => "settings.remote_err_node",
        "host_not_bundled" | "host_not_built" => "settings.remote_err_host_missing",
        "host_startup_failed" | "local_auth_required" => "settings.remote_err_host_start",
        "remote_timeout" | "ipc_timeout_outcome_unknown" => "settings.remote_err_timeout",
        "invalid_pairing" | "invalid_qr" | "expired_pairing" => "settings.remote_err_bad_payload",
        "room_create_429" => "settings.remote_err_relay_busy",
        "proof_of_work_too_hard" => "settings.remote_err_relay_busy",
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
    parse_pairing(&run_remote(
        &["remote", "pair-json", "Phone"],
        Duration::from_secs(35),
    )?)
}

fn load_devices() -> anyhow::Result<Vec<RemoteDevice>> {
    parse_devices(&run_remote(
        &["remote", "devices-json"],
        Duration::from_secs(20),
    )?)
}

fn revoke_device(id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(valid_device_id(id), "device_not_found");
    // ホストが止まっていても CLI がローカルで失効する（リレーへの削除は届かなくてもよい）。
    run_remote(&["remote", "revoke", id], Duration::from_secs(20))?;
    Ok(())
}

/// `necoder remote …` を実行して stdout を返す。失敗は既知のコードだけを持ち出す。
fn run_remote(args: &[&str], timeout: Duration) -> anyhow::Result<Vec<u8>> {
    let mut command = child_process::command(std::env::current_exe()?);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            anyhow::bail!("remote_timeout");
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
    Ok(output)
}

impl SettingsView {
    /// 隔離した UI 撮影用。公開鍵やユーザー設定を変更せず、ダミー QR だけを描画する。
    #[cfg(feature = "remote-preview")]
    pub fn remote_preview(&mut self, payload: &[u8]) {
        self.remote_pairing = parse_pairing(payload).ok();
        self.remote_devices.devices = parse_devices(payload).ok();
        self.remote_preview_only = true;
    }

    /// 「登録済みの端末」を読み直す（スマホ連携のページが描かれている間だけ、5 秒おき）。
    /// 隔離撮影（`remote_preview`）は描画がここへ来る前に返るので、本物のホストを呼ばない。
    pub(super) fn refresh_remote_devices_if_stale(&mut self, cx: &mut Context<Self>) {
        let list = &mut self.remote_devices;
        if list.loading
            || list
                .loaded_at
                .is_some_and(|at| at.elapsed() < DEVICE_REFRESH)
        {
            return;
        }
        list.loading = true;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async { load_devices() })
                .await;
            let updated = view.update(cx, |view, cx| {
                let list = &mut view.remote_devices;
                list.loading = false;
                list.loaded_at = Some(Instant::now());
                match result {
                    Ok(devices) => {
                        list.devices = Some(devices);
                        list.error = None;
                    }
                    Err(error) => {
                        eprintln!("端末の一覧を読み込めない: {error}");
                        list.error = Some(i18n::t!("settings.remote_devices_error").into());
                    }
                }
                cx.notify();
            });
            if updated.is_err() {
                return; // 設定画面が閉じた。
            }
            // 次の描画で読み直す。ページが表示されていなければ描画されないので、そこで止まる。
            cx.background_executor().timer(DEVICE_REFRESH).await;
            if let Err(error) = view.update(cx, |_, cx| cx.notify()) {
                eprintln!("Remote device list closed: {error}");
            }
        })
        .detach();
    }

    fn remove_remote_device(&mut self, id: String, cx: &mut Context<Self>) {
        let list = &mut self.remote_devices;
        if list.removing.is_some() {
            return;
        }
        list.confirming = None;
        list.removing = Some(id.clone());
        list.error = None;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let target = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move { revoke_device(&target) })
                .await;
            if let Err(error) = view.update(cx, |view, cx| {
                let list = &mut view.remote_devices;
                list.removing = None;
                list.loaded_at = None; // すぐ読み直す。
                match result {
                    Ok(()) => {
                        if let Some(devices) = list.devices.as_mut() {
                            devices.retain(|device| device.id != id);
                        }
                        // 表示中の QR の端末なら、その QR も閉じる（もう読み取っても繋がらない）。
                        if view
                            .remote_pairing
                            .as_ref()
                            .is_some_and(|pairing| pairing_room(&pairing.url) == Some(id.as_str()))
                        {
                            view.remote_pairing = None;
                            view.remote_generation += 1;
                        }
                    }
                    Err(error) => {
                        eprintln!("端末を削除できない: {error}");
                        list.error = Some(i18n::t!("settings.remote_device_remove_failed").into());
                    }
                }
                cx.notify();
            }) {
                eprintln!("Remote device list closed: {error}");
            }
        })
        .detach();
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
                    // 発行した QR の「読み取り待ち」をすぐ一覧に出す。
                    view.remote_devices.loaded_at = None;
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
            .child(self.remote_devices_section(cx))
    }

    /// 「登録済みの端末」: 1 台 = 1 枚のカード（種類と状態 / 登録・最終接続・共有数 / 削除）。
    fn remote_devices_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = &self.theme;
        let list = &self.remote_devices;
        let note = |text: String| {
            div()
                .text_size(px(12.))
                .text_color(theme.fg2)
                .child(SharedString::from(text))
        };
        let mut section = div().flex().flex_col().gap(px(6.)).pt(px(10.)).child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg1)
                .child(SharedString::from(i18n::t!(
                    "settings.remote_devices_heading"
                ))),
        );
        if let Some(error) = list.error.clone() {
            section = section.child(note(error.to_string()));
        }
        let Some(devices) = list.devices.as_ref() else {
            return section;
        };
        if devices.is_empty() {
            return section.child(note(i18n::t!("settings.remote_devices_empty")));
        }
        for (index, device) in devices.iter().enumerate() {
            let (dot, status) = match device.status {
                DeviceStatus::Online => (theme.ok, i18n::t!("settings.remote_device_online")),
                DeviceStatus::Offline => (
                    theme.fg2.opacity(0.5),
                    i18n::t!("settings.remote_device_offline"),
                ),
                DeviceStatus::Waiting => (theme.warn, i18n::t!("settings.remote_device_waiting")),
            };
            let name = device
                .model
                .clone()
                .unwrap_or_else(|| i18n::t!("settings.remote_device_phone"));
            let text_button = |id: (&'static str, usize), label: String, strong: bool| {
                div()
                    .id(id)
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(if strong { theme.fg0 } else { theme.fg2 })
                    .when(strong, |element| {
                        element.border_1().border_color(theme.border)
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(SharedString::from(label))
            };
            let actions = if list.removing.as_deref() == Some(device.id.as_str()) {
                div()
                    .text_size(px(12.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.cli_busy")))
            } else if list.confirming.as_deref() == Some(device.id.as_str()) {
                let id = device.id.clone();
                div()
                    .flex()
                    .gap(px(4.))
                    .child(
                        text_button(
                            ("remote-device-cancel", index),
                            i18n::t!("settings.remote_device_cancel"),
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, _, _, cx| {
                                view.remote_devices.confirming = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        text_button(
                            ("remote-device-remove-confirm", index),
                            i18n::t!("settings.remote_device_remove_confirm"),
                            true,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, _, cx| {
                                view.remove_remote_device(id.clone(), cx);
                            }),
                        ),
                    )
            } else {
                let id = device.id.clone();
                div().child(
                    text_button(
                        ("remote-device-remove", index),
                        i18n::t!("settings.remote_device_remove"),
                        false,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            view.remote_devices.confirming = Some(id.clone());
                            cx.notify();
                        }),
                    ),
                )
            };
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(9.))
                    .rounded(px(8.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(theme.fg0)
                                            .child(SharedString::from(name)),
                                    )
                                    .child(div().size(px(7.)).rounded_full().bg(dot))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(theme.fg1)
                                            .child(SharedString::from(status)),
                                    ),
                            )
                            // 狭い幅でも 1 行に収める（「プロジェク / ト」と折り返さない）。
                            .child(
                                note(device_details(device))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis(),
                            ),
                    )
                    .child(div().flex_none().child(actions)),
            );
        }
        section
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

    /// 端末の一覧は形の正しいものだけ受け取る（ID は CLI の引数に渡すので特に厳しく）。
    #[test]
    fn device_list_validation() {
        let room = "a".repeat(43);
        let payload = serde_json::json!({"running": false, "devices": [
            {"id": room, "model": "iPhone", "status": "offline", "paired_at": 1, "last_seen": 2, "projects": ["a", "b"]},
            {"id": "b".repeat(43), "model": "<script>", "status": "waiting", "projects": []},
        ]});
        let devices = parse_devices(payload.to_string().as_bytes()).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].status, DeviceStatus::Offline);
        assert_eq!(devices[0].model.as_deref(), Some("iPhone"));
        assert_eq!(devices[0].projects, 2);
        // 知らない種類は名前に出さない（「スマホ」に倒す）。
        assert_eq!(devices[1].model, None);
        for bad in [
            serde_json::json!({"devices": [{"id": "--help", "status": "online"}]}),
            serde_json::json!({"devices": [{"id": room, "status": "hacked"}]}),
            serde_json::json!({"running": true}),
        ] {
            assert!(parse_devices(bad.to_string().as_bytes()).is_err());
        }
        assert!(parse_devices(br#"{"running":true,"devices":[]}"#)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pairing_url_names_its_device() {
        let url = format!(
            "https://control.necoder.com/#room={}&token=t&secret=s",
            "a".repeat(43)
        );
        assert_eq!(pairing_room(&url), Some("a".repeat(43).as_str()));
        assert_eq!(pairing_room("https://control.necoder.com/"), None);
    }

    /// 読み取り待ちの QR に「登録」「最終接続」は出さない。接続中は「最終接続」を出さない。
    #[test]
    fn device_details_show_only_what_applies() {
        let mut device = RemoteDevice {
            id: "a".repeat(43),
            model: None,
            status: DeviceStatus::Waiting,
            paired_at: Some(now_ms()),
            last_seen: Some(now_ms()),
            projects: 1,
        };
        let projects = i18n::t!("settings.remote_device_projects", "count" => 1);
        assert_eq!(device_details(&device), projects);
        device.status = DeviceStatus::Online;
        assert_eq!(device_details(&device).matches(" · ").count(), 1);
        device.status = DeviceStatus::Offline;
        assert_eq!(device_details(&device).matches(" · ").count(), 2);
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
        // 混雑（429）は「ネットに繋がらない」と原因が違うので別の一文にする。
        assert_ne!(
            failure_message("room_create_429"),
            failure_message("room_create_503")
        );
        assert_eq!(
            failure_message("proof_of_work_too_hard"),
            failure_message("room_create_429")
        );
        // stderr から拾う段でも 429 が総称の "room_create_" に食われない。
        assert_eq!(known_failure("room_create_429"), Some("room_create_429"));
        assert_eq!(known_failure("room_create_503"), Some("room_create_"));
        assert_eq!(failure_message("なにか未知の失敗"), generic);
        // 翻訳漏れならキー文字列がそのまま返る（i18n::translate の仕様）。
        assert!(
            !no_gui.starts_with("settings."),
            "ja/en に対訳が無い: {no_gui}"
        );
    }
}
