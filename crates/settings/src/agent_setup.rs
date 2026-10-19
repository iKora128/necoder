//! 設定の「インストール」「ログイン」を裏で進める（`acp_client::setup` の画面側・2026-10-19・#69）。
//!
//! 押すと、エージェントの行を開いたまま、その中で進み具合を見せる:
//! - **インストール中**: 今の段（Node.js を準備 → 本体をダウンロード → 仕上げ）と経過秒。止めるボタンは
//!   出さない（npm を途中で止めると置き場が半端になる）。終われば一覧を読み直し、未ログインなら
//!   そのままログインへ進む（ボタン 1 つで最後まで）
//! - **ログイン中**: 「ブラウザでログインしてください」・開かなかった時のリンク・Claude Code はブラウザに
//!   出たコードの入力欄。CLI が終われば一覧を読み直す
//! - **失敗**: 理由の 1 行とログの最後の数行・「もう一度」・「ターミナルで実行」（手動の逃げ道）
//!
//! 進み具合は [`TICK`] ごとに描き直す（裏の作業がある間だけ回る見張り 1 本）。

use super::*;
use acp_client::setup::{LoginSession, SetupError, SetupStage};
use acp_client::AgentKind;
use editor_view::ComposerEvent;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 進み具合を描き直す間隔。
const TICK: Duration = Duration::from_millis(400);

/// 裏で進めている途中の様子（エージェントの id ごと）。
pub(crate) enum AgentJob {
    Installing {
        stage: Arc<Mutex<SetupStage>>,
        started: Instant,
    },
    InstallFailed(SetupError),
    LoggingIn {
        session: Arc<Mutex<LoginSession>>,
        code: Entity<EditorView>,
        code_sent: bool,
        started: Instant,
    },
    LoginFailed {
        tail: Vec<String>,
    },
}

impl AgentJob {
    /// まだ動いているか（失敗は止まっている）。
    fn is_running(&self) -> bool {
        matches!(self, AgentJob::Installing { .. } | AgentJob::LoggingIn { .. })
    }
}

/// 「インストール」1 つで入れられるか（このマシン・npm で入れる組み込み）。
pub(crate) fn installs_in_background(kind: &AgentKind, remote: bool) -> bool {
    !remote && acp_client::setup::can_install(kind)
}

/// ログインを裏で進められるか（このマシン・ターミナル無しで動くのを確かめた CLI だけ）。
pub(crate) fn logs_in_in_background(kind: &AgentKind, remote: bool) -> bool {
    !remote && acp_client::setup::can_login_in_background(kind)
}

impl SettingsView {
    pub(crate) fn agent_job(&self, id: &str) -> Option<&AgentJob> {
        self.agent_jobs.get(id)
    }

    /// 「インストール」: Node.js → 本体 → アダプタを裏で入れ、終われば（未ログインなら）ログインへ。
    pub(crate) fn start_agent_setup(&mut self, kind: &'static AgentKind, cx: &mut Context<Self>) {
        if self
            .agent_jobs
            .get(kind.id)
            .is_some_and(AgentJob::is_running)
        {
            return;
        }
        // 押した＝使うつもり（エージェントは既定でオフ・UI-SPEC §12「使う / 使わない」）。
        self.enable_agent_id(kind.id, cx);
        let stage = Arc::new(Mutex::new(SetupStage::Node));
        self.agent_jobs.insert(
            kind.id.to_string(),
            AgentJob::Installing {
                stage: stage.clone(),
                started: Instant::now(),
            },
        );
        self.expanded_agent = Some(SharedString::from(kind.id));
        let registry = self.registry.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    acp_client::setup::install_agent(kind, registry.as_deref(), &|now| {
                        if let Ok(mut current) = stage.lock() {
                            *current = now;
                        }
                    })
                })
                .await;
            let applied = view.update(cx, |view, cx| view.finish_agent_setup(kind, result, cx));
            if let Err(error) = applied {
                eprintln!("{} のインストールの結果を画面へ返せない: {error:#}", kind.label);
            }
        })
        .detach();
        self.ensure_agent_job_ticker(cx);
        cx.notify();
    }

    fn finish_agent_setup(
        &mut self,
        kind: &'static AgentKind,
        result: std::result::Result<(), SetupError>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                self.agent_jobs.remove(kind.id);
                self.refresh_availability(cx);
                // ボタン 1 つで最後まで: 入ったらそのままログインへ（跡も無い時だけ・裏で進められる物だけ）。
                let signed_out =
                    kind.configured_auth_state() == acp_client::AgentAuthState::SignedOut;
                if signed_out && logs_in_in_background(kind, false) {
                    self.start_agent_login(kind, cx);
                }
            }
            Err(error) => {
                self.agent_jobs
                    .insert(kind.id.to_string(), AgentJob::InstallFailed(error));
            }
        }
        cx.notify();
    }

    /// 「ログイン」: CLI のログインを裏で起こす（CLI がブラウザを開く）。
    pub(crate) fn start_agent_login(&mut self, kind: &'static AgentKind, cx: &mut Context<Self>) {
        if self
            .agent_jobs
            .get(kind.id)
            .is_some_and(AgentJob::is_running)
        {
            return;
        }
        // 押した＝使うつもり（エージェントは既定でオフ・UI-SPEC §12「使う / 使わない」）。
        self.enable_agent_id(kind.id, cx);
        let session = match acp_client::setup::start_login(kind) {
            Ok(session) => session,
            Err(error) => {
                self.agent_jobs.insert(
                    kind.id.to_string(),
                    AgentJob::LoginFailed {
                        tail: vec![format!("{error:#}")],
                    },
                );
                cx.notify();
                return;
            }
        };
        let theme = self.theme.clone();
        let accent = self.accent;
        let code = cx.new(|cx| EditorView::plain(theme, accent, true, cx));
        cx.subscribe(&code, move |view, _input, event, cx| {
            if matches!(event, ComposerEvent::Submit) {
                view.send_login_code(kind.id, cx);
            }
        })
        .detach();
        self.agent_jobs.insert(
            kind.id.to_string(),
            AgentJob::LoggingIn {
                session: Arc::new(Mutex::new(session)),
                code,
                code_sent: false,
                started: Instant::now(),
            },
        );
        self.expanded_agent = Some(SharedString::from(kind.id));
        self.ensure_agent_job_ticker(cx);
        cx.notify();
    }

    /// ブラウザに出たコードをログインへ渡す（Claude Code）。
    pub(crate) fn send_login_code(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(AgentJob::LoggingIn {
            session,
            code,
            code_sent,
            ..
        }) = self.agent_jobs.get_mut(id)
        else {
            return;
        };
        let text = code.read(cx).plain_text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let sent = match session.lock() {
            Ok(mut session) => session.send_code(&text).map_err(|error| format!("{error:#}")),
            Err(_) => Err("ログインの状態を読めない".to_string()),
        };
        match sent {
            Ok(()) => *code_sent = true,
            Err(message) => {
                self.agent_jobs
                    .insert(id.to_string(), AgentJob::LoginFailed { tail: vec![message] });
            }
        }
        cx.notify();
    }

    /// 「やめる」（ログインを止める）・失敗の表示を閉じる。インストール中は止めない。
    pub(crate) fn dismiss_agent_job(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.agent_jobs.get(id) {
            Some(AgentJob::Installing { .. }) | None => return,
            Some(AgentJob::LoggingIn { session, .. }) => {
                if let Ok(mut session) = session.lock() {
                    session.cancel();
                }
            }
            Some(AgentJob::InstallFailed(_) | AgentJob::LoginFailed { .. }) => {}
        }
        self.agent_jobs.remove(id);
        cx.notify();
    }

    fn ensure_agent_job_ticker(&mut self, cx: &mut Context<Self>) {
        if self.agent_job_ticker {
            return;
        }
        self.agent_job_ticker = true;
        cx.spawn(async move |view, cx| loop {
            cx.background_executor().timer(TICK).await;
            match view.update(cx, |view, cx| view.tick_agent_jobs(cx)) {
                Ok(true) => continue,
                Ok(false) => break,
                Err(error) => {
                    eprintln!("エージェントの準備の見張りを続けられない（画面が無い）: {error:#}");
                    break;
                }
            }
        })
        .detach();
    }

    /// 進み具合を描き直し、終わったログインを片付ける。戻り値 = まだ見張るか。
    fn tick_agent_jobs(&mut self, cx: &mut Context<Self>) -> bool {
        let mut finished: Vec<(String, bool, Vec<String>)> = Vec::new();
        for (id, job) in &self.agent_jobs {
            if let AgentJob::LoggingIn { session, .. } = job {
                if let Ok(mut session) = session.lock() {
                    if let Some(succeeded) = session.poll() {
                        finished.push((id.clone(), succeeded, session.output_tail()));
                    }
                }
            }
        }
        for (id, succeeded, tail) in finished {
            if succeeded {
                self.agent_jobs.remove(&id);
                self.refresh_availability(cx);
            } else {
                self.agent_jobs.insert(id, AgentJob::LoginFailed { tail });
            }
        }
        let running = self.agent_jobs.values().any(AgentJob::is_running);
        if !running {
            self.agent_job_ticker = false;
        }
        cx.notify();
        running
    }

    /// 行の 2 行目に出す、裏の作業の一言（作業が無ければ `None`）。
    pub(crate) fn agent_job_status(&self, kind: &AgentKind) -> Option<String> {
        Some(match self.agent_jobs.get(kind.id)? {
            AgentJob::Installing { .. } => i18n::t!("settings.agent_job_installing_short"),
            AgentJob::LoggingIn { .. } => i18n::t!("settings.agent_job_logging_in_short"),
            AgentJob::InstallFailed(_) => i18n::t!("settings.agent_job_install_failed_short"),
            AgentJob::LoginFailed { .. } => i18n::t!("settings.agent_job_login_failed_short"),
        })
    }

    /// 開いた準備中の行の、手順の下に出す進み具合（作業が無ければ `None`）。
    pub(crate) fn agent_job_view(
        &self,
        index: usize,
        kind: &'static AgentKind,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = self.theme.clone();
        let code_font = ui::code_font(cx);
        let job = self.agent_jobs.get(kind.id)?;
        let line = |text: String, color: Hsla| {
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(11.5))
                .text_color(color)
                .child(SharedString::from(text))
        };
        let note = |text: String| {
            div()
                .text_size(px(10.5))
                .text_color(theme.fg2)
                .child(SharedString::from(text))
        };
        let log_lines = |lines: &[String]| {
            div()
                .flex()
                .flex_col()
                .px(px(8.))
                .py(px(5.))
                .rounded(px(5.))
                .bg(theme.bg1)
                .border_1()
                .border_color(theme.border)
                .font_family(code_font.clone())
                .text_size(px(10.))
                .text_color(theme.fg2)
                .children(lines.iter().map(|line| {
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(SharedString::from(line.clone()))
                }))
        };
        let panel = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pl(px(28.))
            .pt(px(2.));
        let element = match job {
            AgentJob::Installing { stage, started } => {
                let stage = stage.lock().map(|stage| *stage).unwrap_or(SetupStage::Node);
                let doing = match stage {
                    SetupStage::Node => i18n::t!("settings.agent_job_stage_node"),
                    SetupStage::Agent => {
                        i18n::t!("settings.agent_job_stage_agent", "name" => kind.label)
                    }
                    SetupStage::Adapter => i18n::t!("settings.agent_job_stage_adapter"),
                };
                let seconds = started.elapsed().as_secs();
                panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(spinner(started.elapsed().as_millis() / 400, &theme))
                            .child(line(
                                i18n::t!(
                                    "settings.agent_job_installing",
                                    "doing" => doing,
                                    "seconds" => seconds
                                ),
                                theme.fg0,
                            )),
                    )
                    .child(note(i18n::t!("settings.agent_job_installing_note")))
            }
            AgentJob::InstallFailed(error) => panel
                .child(line(
                    i18n::t!("settings.agent_job_install_failed", "reason" => error.message.clone()),
                    theme.fg0,
                ))
                .when(!error.log_tail.is_empty(), |panel| {
                    panel.child(log_lines(&error.log_tail))
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(
                            self.job_button(("agent-job-retry", index), i18n::t!("settings.agent_job_retry"), true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _, _window, cx| {
                                        cx.stop_propagation();
                                        view.agent_jobs.remove(kind.id);
                                        view.start_agent_setup(kind, cx);
                                    }),
                                ),
                        )
                        .child(
                            self.job_button(
                                ("agent-job-terminal", index),
                                i18n::t!("settings.agent_job_use_terminal"),
                                false,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _, _window, cx| {
                                    cx.stop_propagation();
                                    view.agent_jobs.remove(kind.id);
                                    view.run_terminal_fallback(kind, kind.install_cmd, cx);
                                }),
                            ),
                        )
                        .child(self.dismiss_button(index, kind, cx)),
                ),
            AgentJob::LoggingIn {
                session,
                code,
                code_sent,
                started,
            } => {
                let (url, asks_for_code) = session
                    .lock()
                    .map(|session| (session.url(), session.asks_for_code()))
                    .unwrap_or((None, false));
                let waiting_for_code = asks_for_code && !code_sent;
                let mut panel = panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(spinner(
                            started.elapsed().as_millis() / 400,
                            &theme,
                        ))
                        .child(line(
                            if *code_sent {
                                i18n::t!("settings.agent_job_login_checking")
                            } else {
                                i18n::t!("settings.agent_job_login_browser")
                            },
                            theme.fg0,
                        )),
                );
                if waiting_for_code {
                    let field = div()
                        .flex_1()
                        .min_w_0()
                        .h(px(28.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg0)
                        .overflow_hidden()
                        .child(code.clone());
                    panel = panel
                        .child(note(i18n::t!("settings.agent_job_login_paste")))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(field)
                                .child(
                                    self.job_button(
                                        ("agent-job-send-code", index),
                                        i18n::t!("settings.agent_job_login_send"),
                                        true,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |view, _, _window, cx| {
                                            cx.stop_propagation();
                                            view.send_login_code(kind.id, cx);
                                        }),
                                    ),
                                ),
                        );
                }
                let mut actions = div().flex().flex_wrap().items_center().gap(px(10.));
                if let Some(url) = url {
                    actions = actions.child(
                        self.detail_link(
                            ("agent-job-open-browser", index),
                            i18n::t!("settings.agent_job_login_open_browser"),
                        )
                        .text_size(px(11.))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_, _, _window, cx| {
                                cx.stop_propagation();
                                cx.emit(SettingsViewEvent::OpenUrl(url.clone()));
                            }),
                        ),
                    );
                }
                actions = actions.child(
                    self.detail_link(
                        ("agent-job-cancel", index),
                        i18n::t!("settings.agent_job_cancel"),
                    )
                    .text_size(px(11.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            cx.stop_propagation();
                            view.dismiss_agent_job(kind.id, cx);
                        }),
                    ),
                );
                panel.child(actions)
            }
            AgentJob::LoginFailed { tail } => panel
                .child(line(i18n::t!("settings.agent_job_login_failed"), theme.fg0))
                .when(!tail.is_empty(), |panel| panel.child(log_lines(tail)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(
                            self.job_button(("agent-job-retry", index), i18n::t!("settings.agent_job_retry"), true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _, _window, cx| {
                                        cx.stop_propagation();
                                        view.agent_jobs.remove(kind.id);
                                        view.start_agent_login(kind, cx);
                                    }),
                                ),
                        )
                        .child(
                            self.job_button(
                                ("agent-job-terminal", index),
                                i18n::t!("settings.agent_job_use_terminal"),
                                false,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _, _window, cx| {
                                    cx.stop_propagation();
                                    view.agent_jobs.remove(kind.id);
                                    view.run_terminal_fallback(kind, kind.login_cmd, cx);
                                }),
                            ),
                        )
                        .child(self.dismiss_button(index, kind, cx)),
                ),
        };
        Some(element.into_any_element())
    }

    /// 開発用（実機の確かめ）: AI エージェントのページでその組み込みの「インストール」を押したのと同じ。
    #[cfg(debug_assertions)]
    pub fn debug_start_agent_setup(&mut self, id: &str, cx: &mut Context<Self>) {
        self.select_page(crate::SettingsPage::Agents, cx);
        if let Some(kind) = acp_client::AGENTS.iter().find(|kind| kind.id == id) {
            self.start_agent_setup(kind, cx);
        }
    }

    /// 開発用（実機の確かめ）: その組み込みの「ログイン」を押したのと同じ（行を開いて見せる）。
    #[cfg(debug_assertions)]
    pub fn debug_start_agent_login(&mut self, id: &str, cx: &mut Context<Self>) {
        self.select_page(crate::SettingsPage::Agents, cx);
        if let Some(kind) = acp_client::AGENTS.iter().find(|kind| kind.id == id) {
            self.start_agent_login(kind, cx);
        }
    }

    /// 手動の逃げ道: 今までどおりコマンドをターミナルで流す（変化の見張り付き）。
    fn run_terminal_fallback(
        &mut self,
        kind: &'static AgentKind,
        command: &'static str,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = acp_client::AGENTS
            .iter()
            .position(|known| known.id == kind.id)
        {
            self.run_agent_command(index, command, cx);
        }
    }

    /// 作業の中のボタン（`primary` = 次にやること＝bg3 の面・それ以外は枠だけ）。識別色は使わない。
    fn job_button(&self, id: (&'static str, usize), text: String, primary: bool) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .flex_none()
            .h(px(24.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.border)
            .when(primary, |button| button.bg(theme.bg3).text_color(theme.fg0))
            .when(!primary, |button| button.text_color(theme.fg1))
            .text_size(px(11.5))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|style| style.border_color(theme.fg2).text_color(theme.fg0))
            .child(SharedString::from(text))
    }

    fn dismiss_button(
        &self,
        index: usize,
        kind: &'static AgentKind,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.detail_link(
            ("agent-job-dismiss", index),
            i18n::t!("settings.agent_job_dismiss"),
        )
        .text_size(px(11.))
        .self_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, _, _window, cx| {
                cx.stop_propagation();
                view.dismiss_agent_job(kind.id, cx);
            }),
        )
    }
}

/// 回っている印（色相なし・fg1）。`step` が進むたびに形が変わる（描き直しは [`TICK`] ごと）。
fn spinner(step: u128, theme: &Theme) -> Div {
    const FRAMES: [&str; 4] = ["◐", "◓", "◑", "◒"];
    div()
        .flex_none()
        .w(px(14.))
        .text_size(px(12.))
        .text_color(theme.fg1)
        .child(FRAMES[(step % FRAMES.len() as u128) as usize])
}
