//! 設定 › コネクタの「サービス」一覧と詳細のダイアログ（`mock/settings-connectors.html`）。
//!
//! 一覧は `acp_client::connector` が Claude Code に聞いた結果で、**モデルは呼ばない**。
//! 権限は道具ごとに `connectors.<key>.tools` へ書く。種類ごとの 3 段階は「その種類の道具を全部変える」
//! 操作で、種類の設定を別に持たない（新しく増えた道具は黙って通らず「確認」になる）。

use crate::*;
use acp_client::connector::{Connector, ConnectorState, ConnectorTool, ToolAccess};
use settings_core::{connector_key, ConnectorSetting, ConnectorToolLevel};

/// claude.ai のコネクタ設定の入口。
const CLAUDE_CONNECTORS_URL: &str = "https://claude.ai/settings/connectors";

fn service_key(connector: &Connector) -> String {
    connector
        .codex_app_id
        .as_ref()
        .map(|id| format!("codex_app:{id}"))
        .unwrap_or_else(|| connector_key(&connector.name))
}

const LEVELS: [ConnectorToolLevel; 3] = [
    ConnectorToolLevel::Allow,
    ConnectorToolLevel::Ask,
    ConnectorToolLevel::Block,
];

/// コネクタの一覧の状態。
#[derive(Default)]
pub(crate) struct ConnectorsState {
    pub(crate) services: Vec<Connector>,
    pub(crate) checking: bool,
    /// 一度でも確かめ始めたか（ページを開くたびに `claude` を起こさない）。
    pub(crate) requested: bool,
    pub(crate) failed: bool,
    /// 開いているダイアログのコネクタ（`Connector::name`）。
    pub(crate) detail: Option<String>,
    /// ダイアログで道具を並べている種類。
    pub(crate) expanded: Option<ToolAccess>,
}

/// ロゴ（Simple Icons・CC0）とブランド色。無いサービスは汎用のサーバのアイコン。
fn service_logo(connector: &Connector) -> (&'static str, Option<u32>) {
    match connector.display_name() {
        "Gmail" => ("icons/brand-gmail.svg", Some(0xEA_43_35)),
        "Google Calendar" => ("icons/brand-googlecalendar.svg", Some(0x42_85_F4)),
        "Google Drive" => ("icons/brand-googledrive.svg", Some(0x42_85_F4)),
        "Supabase" => ("icons/brand-supabase.svg", Some(0x3F_CF_8E)),
        "Figma" => ("icons/brand-figma.svg", Some(0xF2_4E_1E)),
        "Stripe" => ("icons/brand-stripe.svg", Some(0x63_5B_FF)),
        "Draw io MCP" => ("icons/brand-drawio.svg", Some(0xF0_87_05)),
        "Claude Docs" => ("icons/brand-claude.svg", Some(CLAUDE_BRAND)),
        _ => ("icons/server.svg", None),
    }
}

const CLAUDE_BRAND: u32 = 0xD9_77_57;

fn access_label(access: ToolAccess) -> (String, String) {
    match access {
        ToolAccess::Read => (
            i18n::t!("settings.connectors_access_read"),
            i18n::t!("settings.connectors_access_read_sub"),
        ),
        ToolAccess::Write => (
            i18n::t!("settings.connectors_access_write"),
            i18n::t!("settings.connectors_access_write_sub"),
        ),
        ToolAccess::Delete => (
            i18n::t!("settings.connectors_access_delete"),
            i18n::t!("settings.connectors_access_delete_sub"),
        ),
        ToolAccess::Send => (
            i18n::t!("settings.connectors_access_send"),
            i18n::t!("settings.connectors_access_send_sub"),
        ),
    }
}

fn level_label(level: ConnectorToolLevel) -> String {
    match level {
        ConnectorToolLevel::Allow => i18n::t!("settings.connectors_level_allow"),
        ConnectorToolLevel::Ask => i18n::t!("settings.connectors_level_ask"),
        ConnectorToolLevel::Block => i18n::t!("settings.connectors_level_block"),
    }
}

/// 道具の今の権限。書いていなければ「確認」。
fn tool_level(setting: Option<&ConnectorSetting>, tool: &str) -> ConnectorToolLevel {
    setting
        .and_then(|setting| setting.tools.get(tool).copied())
        .unwrap_or_default()
}

/// 種類全体の権限。道具ごとにばらばらなら `None`（どの段も選ばれていない表示にする）。
fn group_level(
    setting: Option<&ConnectorSetting>,
    tools: &[&ConnectorTool],
) -> Option<ConnectorToolLevel> {
    let first = tool_level(setting, &tools.first()?.name);
    tools
        .iter()
        .all(|tool| tool_level(setting, &tool.name) == first)
        .then_some(first)
}

impl SettingsView {
    /// ナビの件数に足す分（接続済みのコネクタの数・そのうち有効な数）。
    pub(crate) fn connector_counts(&self, settings: &Settings) -> (usize, usize) {
        let connected: Vec<&Connector> = self
            .connectors
            .services
            .iter()
            .filter(|connector| connector.state == ConnectorState::Connected)
            .collect();
        let enabled = connected
            .iter()
            .filter(|connector| {
                settings
                    .connectors
                    .get(&service_key(&connector))
                    .is_none_or(|setting| setting.enabled)
            })
            .count();
        (connected.len(), enabled)
    }

    /// 一覧をまだ確かめていなければ、背景で確かめ始める（ページを初めて描く時）。
    pub(crate) fn ensure_connectors(&mut self, cx: &mut Context<Self>) {
        if !self.connectors.requested {
            self.refresh_connectors(cx);
        }
    }

    pub(crate) fn refresh_connectors(&mut self, cx: &mut Context<Self>) {
        if self.connectors.checking {
            return;
        }
        self.connectors.requested = true;
        self.connectors.checking = true;
        self.connectors.failed = false;
        cx.notify();
        let codex_env = get(cx).agent_servers.get("codex").map(|server| server.env().clone()).unwrap_or_default();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let claude = std::thread::spawn(acp_client::connector::probe_claude_connectors);
                    let codex = acp_client::connector::probe_codex_connectors(&codex_env);
                    let claude = claude.join().unwrap_or_else(|_| {
                        Err(anyhow::anyhow!("Claude の接続確認が終了しました"))
                    });
                    match (claude, codex) {
                        (Ok(mut services), Ok(apps)) => {
                            services.extend(apps);
                            Ok(services)
                        }
                        (Ok(services), Err(error)) | (Err(error), Ok(services)) => {
                            eprintln!("一方の AI のコネクタを確認できません: {error:#}");
                            Ok(services)
                        }
                        (Err(error), Err(_)) => Err(error),
                    }
                })
                .await;
            view.update(cx, |view, cx| {
                view.connectors.checking = false;
                match result {
                    Ok(services) => view.connectors.services = services,
                    Err(error) => {
                        eprintln!("コネクタを確かめられません: {error:#}");
                        view.connectors.failed = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_connector_enabled(&mut self, name: &str, enabled: bool, cx: &mut Context<Self>) {
        let key = self
            .connectors
            .services
            .iter()
            .find(|service| service.name == name)
            .map(service_key)
            .unwrap_or_else(|| connector_key(name));
        let result = write_user_file(cx, |path| {
            settings_core::persist_connector_enabled(path, &key, enabled)
        });
        self.report_save(result, cx);
        cx.notify();
    }

    fn set_connector_tools(
        &mut self,
        name: &str,
        tools: Vec<String>,
        level: ConnectorToolLevel,
        cx: &mut Context<Self>,
    ) {
        let key = self
            .connectors
            .services
            .iter()
            .find(|service| service.name == name)
            .map(service_key)
            .unwrap_or_else(|| connector_key(name));
        let result = write_user_file(cx, |path| {
            settings_core::persist_connector_tools(path, &key, &tools, level)
        });
        self.report_save(result, cx);
        cx.notify();
    }

    fn open_connector(&mut self, name: String, cx: &mut Context<Self>) {
        self.connectors.detail = Some(name);
        self.connectors.expanded = None;
        cx.notify();
    }

    fn close_connector(&mut self, cx: &mut Context<Self>) {
        if self.connectors.detail.take().is_some() {
            cx.notify();
        }
    }

    /// 開発用（offscreen 検証）: コネクタの詳細を開いた状態にする。
    #[cfg(debug_assertions)]
    pub fn debug_open_connector(&mut self, name: &str, cx: &mut Context<Self>) {
        self.open_connector(name.to_string(), cx);
    }

    fn connector_logo(&self, connector: &Connector) -> Div {
        let (path, brand) = service_logo(connector);
        let color = brand.map_or(self.theme.fg2, |brand| gpui::rgb(brand).into());
        div()
            .flex_none()
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .child(svg().path(path).size(px(20.)).text_color(color))
    }

    /// この接続を使う AI の印。
    fn connector_ai_badge(&self, connector: &Connector) -> Div {
        div()
            .flex_none()
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .bg(self.theme.bg3)
            .child(
                svg()
                    .path(if connector.codex_app_id.is_some() {
                        "icons/server.svg"
                    } else {
                        "icons/brand-claude.svg"
                    })
                    .size(px(14.))
                    .text_color(gpui::rgb(CLAUDE_BRAND)),
            )
    }

    fn connector_action(&self, id: (&'static str, usize), label: String) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .flex_none()
            .h(px(22.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
            .child(SharedString::from(label))
    }

    /// 「サービス」の見出しと一覧。
    pub(crate) fn connectors_section(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let heading = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mt(px(6.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg1)
                    .child(SharedString::from(i18n::t!("settings.connectors_services"))),
            )
            .child(
                div()
                    .id("connectors-recheck")
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0))
                    .child(SharedString::from(if self.connectors.checking {
                        i18n::t!("settings.connectors_checking")
                    } else {
                        i18n::t!("settings.connectors_recheck")
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _window, cx| view.refresh_connectors(cx)),
                    ),
            );
        let body = if self.connectors.services.is_empty() {
            let message = if self.connectors.failed {
                i18n::t!("settings.connectors_failed")
            } else if self.connectors.checking {
                i18n::t!("settings.connectors_checking")
            } else {
                i18n::t!("settings.connectors_empty")
            };
            div()
                .px(px(12.))
                .py(px(9.))
                .rounded(px(8.))
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.5))
                .text_color(theme.fg2)
                .child(SharedString::from(message))
        } else {
            let mut list = div()
                .flex()
                .flex_col()
                .rounded(px(9.))
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden();
            for (index, connector) in self.connectors.services.iter().enumerate() {
                list = list.child(self.connector_row(index, connector, settings, cx));
            }
            list
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(heading)
            .child(body)
    }

    fn connector_row(
        &self,
        index: usize,
        connector: &Connector,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let name = connector.name.clone();
        let setting = settings.connectors.get(&service_key(&connector));
        let enabled = setting.is_none_or(|setting| setting.enabled);
        let connected = connector.state == ConnectorState::Connected;
        let dimmed = !connected || !enabled;
        let sub = match connector.state {
            ConnectorState::Connected => i18n::t!(
                if connector.codex_app_id.is_some() { "settings.connectors_codex_tools" } else { "settings.connectors_connected" },
                "count" => connector.tools.len()
            ),
            ConnectorState::NeedsAuth => i18n::t!("settings.connectors_needs_login"),
            ConnectorState::Pending => i18n::t!("settings.connectors_pending"),
            ConnectorState::Failed => i18n::t!("settings.connectors_unavailable"),
        };
        let row = div()
            .id(("connector-row", index))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(9.))
            .when(index > 0, |row| row.border_t_1().border_color(theme.border))
            .child(self.connector_logo(connector))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if dimmed { theme.fg1 } else { theme.fg0 })
                            .child(SharedString::from(connector.display_name().to_string())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(if connector.state == ConnectorState::NeedsAuth {
                                theme.fg0
                            } else {
                                theme.fg2
                            })
                            .child(SharedString::from(sub)),
                    ),
            );
        if connected {
            let toggle_name = name.clone();
            row.cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| view.open_connector(name.clone(), cx)),
                )
                .child(self.connector_ai_badge(connector))
                .child(
                    self.switch(("connector-switch", index), enabled)
                        .flex_none()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, _window, cx| {
                                cx.stop_propagation();
                                view.set_connector_enabled(&toggle_name, !enabled, cx);
                            }),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(14.))
                        .text_size(px(13.))
                        .text_color(theme.fg2)
                        .child("›"),
                )
                .into_any_element()
        } else {
            row.child(
                self.connector_action(
                    ("connector-login", index),
                    i18n::t!("settings.connectors_login"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_view, _, _window, cx| {
                        cx.emit(SettingsViewEvent::OpenUrl(
                            CLAUDE_CONNECTORS_URL.to_string(),
                        ));
                    }),
                ),
            )
            .into_any_element()
        }
    }

    /// 3 段階の切り替え。`selected` が `None` なら道具ごとにばらばら。
    fn level_segments(
        &self,
        id: &'static str,
        id_base: usize,
        selected: Option<ConnectorToolLevel>,
        name: &str,
        tools: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let mut segments = div()
            .flex_none()
            .flex()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .overflow_hidden();
        for (index, level) in LEVELS.into_iter().enumerate() {
            let chosen = selected == Some(level);
            let name = name.to_string();
            let tools = tools.clone();
            segments = segments.child(
                div()
                    .id((id, id_base * LEVELS.len() + index))
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .when(index > 0, |segment| {
                        segment.border_l_1().border_color(theme.border)
                    })
                    .text_size(px(10.5))
                    .cursor_pointer()
                    .when(chosen, |segment| {
                        segment
                            .bg(theme.bg3)
                            .text_color(theme.fg0)
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!chosen, |segment| {
                        segment
                            .text_color(theme.fg2)
                            .hover(|style| style.text_color(theme.fg0))
                    })
                    .child(SharedString::from(level_label(level)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            cx.stop_propagation();
                            view.set_connector_tools(&name, tools.clone(), level, cx);
                        }),
                    ),
            );
        }
        segments
    }

    /// コネクタの詳細（設定の面の上・中央）。
    pub(crate) fn render_connector_detail(
        &self,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let detail = self.connectors.detail.as_ref()?;
        let connector = self
            .connectors
            .services
            .iter()
            .find(|connector| &connector.name == detail)?
            .clone();
        let theme = self.theme.clone();
        let setting = settings.connectors.get(&service_key(&connector));
        let section_title = |title: String, sub: Option<String>| {
            div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .text_size(px(11.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.fg1)
                        .child(SharedString::from(title)),
                )
                .children(sub.map(|sub| {
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(sub))
                }))
        };
        let box_style = || {
            div()
                .flex()
                .flex_col()
                .rounded(px(8.))
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
        };

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .py(px(12.))
            .border_b_1()
            .border_color(theme.border)
            .child(self.connector_logo(&connector))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(connector.display_name().to_string())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!(
                                "settings.connectors_detail_sub",
                                "host" => connector.host().unwrap_or(""),
                                "count" => connector.tools.len()
                            ))),
                    ),
            )
            .child(
                div()
                    .id("connector-close")
                    .flex_none()
                    .px(px(4.))
                    .text_size(px(15.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0))
                    .child("✕")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _window, cx| view.close_connector(cx)),
                    ),
            );

        let claude_row = box_style().child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(8.))
                .child(
                    svg()
                        .path(if connector.codex_app_id.is_some() {
                            "icons/server.svg"
                        } else {
                            "icons/brand-claude.svg"
                        })
                        .flex_none()
                        .size(px(16.))
                        .text_color(gpui::rgb(CLAUDE_BRAND)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(12.)).text_color(theme.fg0).child(
                            if connector.codex_app_id.is_some() {
                                "Codex"
                            } else {
                                "Claude"
                            },
                        ))
                        .child(div().text_size(px(10.5)).text_color(theme.fg2).child(
                            SharedString::from(i18n::t!(if connector.codex_app_id.is_some() {
                                "settings.connectors_codex_connected"
                            } else {
                                "settings.connectors_claude_connected"
                            })),
                        )),
                )
                .child(
                    self.connector_action(
                        ("connector-manage", 0),
                        i18n::t!(if connector.codex_app_id.is_some() { "settings.connectors_codex_manage" } else { "settings.connectors_manage" }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let url = if connector.codex_app_id.is_some() {
                                "https://chatgpt.com/apps"
                            } else {
                                CLAUDE_CONNECTORS_URL
                            }
                            .to_string();
                            move |_view, _, _window, cx| {
                                cx.emit(SettingsViewEvent::OpenUrl(url.clone()));
                            }
                        }),
                    ),
                ),
        );

        let mut permissions = box_style();
        let mut row_index = 0usize;
        for (group_index, access) in ToolAccess::ALL.into_iter().enumerate() {
            let tools: Vec<&ConnectorTool> = connector
                .tools
                .iter()
                .filter(|tool| tool.access == access)
                .collect();
            if tools.is_empty() {
                continue;
            }
            let (label, sub) = access_label(access);
            let names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
            let open = self.connectors.expanded == Some(access);
            let segments = self.level_segments(
                "connector-group-level",
                group_index,
                group_level(setting, &tools),
                &connector.name,
                names,
                cx,
            );
            permissions = permissions.child(
                div()
                    .id(("connector-group", group_index))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(8.))
                    .cursor_pointer()
                    .when(row_index > 0, |row| {
                        row.border_t_1().border_color(theme.border)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.connectors.expanded =
                                (view.connectors.expanded != Some(access)).then_some(access);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(12.))
                            .text_color(theme.fg2)
                            .child(if open { "⌄" } else { "›" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(6.))
                                    .text_size(px(12.))
                                    .text_color(theme.fg0)
                                    .child(SharedString::from(label))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(theme.fg2)
                                            .child(SharedString::from(tools.len().to_string())),
                                    ),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(10.5))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(sub)),
                            ),
                    )
                    .child(segments),
            );
            row_index += 1;
            if open {
                let mut list = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .py(px(4.))
                    .bg(theme.bg1)
                    .border_t_1()
                    .border_color(theme.border);
                for (tool_index, tool) in tools.iter().enumerate() {
                    let segments = self.level_segments(
                        "connector-tool-level",
                        group_index * 1000 + tool_index,
                        Some(tool_level(setting, &tool.name)),
                        &connector.name,
                        vec![tool.name.clone()],
                        cx,
                    );
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .pl(px(32.))
                            .pr(px(10.))
                            .py(px(2.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(ui::code_font(cx))
                                    .text_size(px(11.))
                                    .text_color(theme.fg1)
                                    .child(SharedString::from(tool.name.clone())),
                            )
                            .child(segments),
                    );
                }
                permissions = permissions.child(list);
            }
        }

        let card = div()
            .w(px(600.))
            .max_h(px(640.))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(12.),
                gpui::hsla(0., 0., 0., 0.5),
            )
            .blur_radius(px(28.))])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(
                div()
                    .id("connector-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(16.))
                    .py(px(14.))
                    .child(section_title(
                        i18n::t!("settings.connectors_available_ai"),
                        None,
                    ))
                    .child(claude_row)
                    .child(div().h(px(6.)))
                    .child(section_title(
                        i18n::t!("settings.connectors_permissions"),
                        Some(i18n::t!("settings.connectors_permissions_sub")),
                    ))
                    .child(permissions)
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("settings.connectors_note"))),
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
                    cx.listener(|view, _, _window, cx| view.close_connector(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ConnectorTool {
        ConnectorTool {
            name: name.to_string(),
            access: ToolAccess::Read,
        }
    }

    #[test]
    fn a_group_shows_a_level_only_when_every_tool_agrees() {
        let (first, second) = (tool("get_thread"), tool("search_threads"));
        let tools = vec![&first, &second];
        assert_eq!(
            group_level(None, &tools),
            Some(ConnectorToolLevel::Ask),
            "書いていなければ確認"
        );
        let mut setting = ConnectorSetting::default();
        setting
            .tools
            .insert("get_thread".to_string(), ConnectorToolLevel::Allow);
        assert_eq!(group_level(Some(&setting), &tools), None, "ばらばら");
        setting
            .tools
            .insert("search_threads".to_string(), ConnectorToolLevel::Allow);
        assert_eq!(
            group_level(Some(&setting), &tools),
            Some(ConnectorToolLevel::Allow)
        );
    }
}
