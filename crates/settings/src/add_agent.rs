//! エージェントを追加（H2・issue #38）。設定 › AI エージェントの一覧の下の「＋ エージェントを追加」で、
//! ACP の公開レジストリの全件を名前・説明・配布の形（binary / npx / uvx）・対応 OS で並べ、検索して
//! 足す。足すと settings.json の `agent_servers.<レジストリの id>` に `{"type": "registry", "name": …}`
//! を書く（新しいキーは作らない）。足した物は一覧に並び、使う / 使わない・既定・起動の変更は
//! 組み込みと同じ行で扱う（`agent_launch`）。
//!
//! レジストリは開いた時に**キャッシュを読むだけ**（ネットワークへは行かない）。キャッシュが無い・
//! 古い時は「取得する」「取り直す」で人が頼んだ時だけ取りに行く（取得先はレジストリの URL だけ・
//! `acp_client::registry`）。

use super::*;
use gpui::Focusable as _;
use std::sync::Arc;

/// 開いている「エージェントを追加」のダイアログ。
pub(crate) struct AddAgentDialog {
    pub(crate) search: Entity<EditorView>,
    /// 検索語の写し（打つたびに入れ替わる）。
    pub(crate) query: String,
    /// 取り直している最中（連打防止・「取得しています…」表示）。
    pub(crate) fetching: bool,
    /// 直近の取り直しの失敗。
    pub(crate) fetch_error: Option<SharedString>,
    _search_subscription: gpui::Subscription,
}

/// OS / arch の表示名（レジストリのキー `darwin-aarch64` を人の言葉へ）。
fn platform_summary(agent: &acp_client::registry::RegistryAgent) -> String {
    use acp_client::registry::DistributionKind;
    let kinds = agent.kinds();
    // npx / uvx は OS を問わない（node / uv が要るだけ）。
    if kinds.contains(&DistributionKind::Npx) || kinds.contains(&DistributionKind::Uvx) {
        return i18n::t!("settings.add_agent_os_any");
    }
    let platforms = agent.binary_platforms();
    let mut parts = Vec::new();
    for (os, label) in [
        ("darwin", "macOS"),
        ("linux", "Linux"),
        ("windows", "Windows"),
    ] {
        let archs: Vec<&str> = platforms
            .iter()
            .filter_map(|platform| platform.strip_prefix(os)?.strip_prefix('-'))
            .map(|arch| match arch {
                "aarch64" => "arm64",
                "x86_64" => "x64",
                other => other,
            })
            .collect();
        match archs.len() {
            0 => {}
            // 両方ある時は OS だけ（arch を並べても読む手間が増えるだけ）。
            2.. => parts.push(label.to_string()),
            _ => parts.push(format!("{label}（{}）", archs.join(", "))),
        }
    }
    i18n::t!("settings.add_agent_os", "os" => parts.join(" · "))
}

/// このマシンでの見込みの注記（足す前に分かること）と、注意を引く文か（`true` = fg1 で出す）。
/// binary は検証の値の有無を必ず出す（無い物は「検証の値がありません」・H2-b）。
fn plan_note(
    result: &Result<acp_client::LaunchPlan, acp_client::LaunchError>,
) -> Option<(String, bool)> {
    match result {
        Ok(acp_client::LaunchPlan::Binary { verified: true, .. }) => {
            Some((i18n::t!("settings.add_agent_sha256"), false))
        }
        Ok(acp_client::LaunchPlan::Binary {
            verified: false, ..
        }) => Some((i18n::t!("settings.add_agent_no_sha256"), true)),
        Ok(_) => None,
        Err(error) => Some((launch_problem_text(error), true)),
    }
}

/// 起動できない理由を人の言葉へ（設定の行と追加の画面で同じ文）。
pub(crate) fn launch_problem_text(error: &acp_client::LaunchError) -> String {
    match error {
        acp_client::LaunchError::NotInRegistry { id } => {
            i18n::t!("settings.custom_not_in_registry", "id" => id)
        }
        acp_client::LaunchError::NoDistribution { platform } => i18n::t!(
            "settings.custom_no_distribution",
            "platform" => platform.as_deref().unwrap_or("?")
        ),
        acp_client::LaunchError::NeedsNode => i18n::t!("settings.custom_needs_node"),
        acp_client::LaunchError::NotSupportedYet(kind) => {
            i18n::t!("settings.custom_not_yet", "kind" => kind.as_str())
        }
        acp_client::LaunchError::BinaryOnRemote => i18n::t!("settings.custom_binary_on_remote"),
        acp_client::LaunchError::NotDeployed => i18n::t!("settings.custom_not_deployed"),
        acp_client::LaunchError::Deploy(error) => {
            i18n::t!("settings.custom_deploy_failed", "reason" => error.to_string())
        }
    }
}

impl SettingsView {
    /// 「＋ エージェントを追加」: 検索欄にフォーカスしてダイアログを開く。レジストリは手元の写しを使う。
    pub(crate) fn open_add_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme.clone();
        let accent = self.accent;
        let search = cx.new(|cx| EditorView::plain(theme, accent, true, cx));
        let subscription = cx.observe(&search, |view, search, cx| {
            let query = search.read(cx).plain_text();
            if let Some(dialog) = view.add_agent.as_mut() {
                if dialog.query != query {
                    dialog.query = query;
                    cx.notify();
                }
            }
        });
        window.focus(&search.read(cx).focus_handle(cx), cx);
        if self.registry.is_none() {
            self.registry = acp_client::registry::load_cached().map(Arc::new);
        }
        self.add_agent = Some(AddAgentDialog {
            search,
            query: String::new(),
            fetching: false,
            fetch_error: None,
            _search_subscription: subscription,
        });
        cx.notify();
    }

    /// 開発用（offscreen 検証）: AI エージェントのページで「エージェントを追加」を開き、検索語を入れる。
    #[cfg(debug_assertions)]
    pub fn debug_open_add_agent(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_page(SettingsPage::Agents, cx);
        self.open_add_agent(window, cx);
        if let Some(search) = self.add_agent.as_ref().map(|dialog| dialog.search.clone()) {
            search.update(cx, |search, cx| search.set_plain_text(query, cx));
        }
    }

    pub(crate) fn close_add_agent(&mut self, cx: &mut Context<Self>) {
        if self.add_agent.take().is_some() {
            cx.notify();
        }
    }

    /// 「取得する」/「取り直す」: レジストリを取りに行き、キャッシュへ書く（背景・人が頼んだ時だけ）。
    pub(crate) fn fetch_registry(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.add_agent.as_mut() else {
            return;
        };
        if dialog.fetching {
            return;
        }
        dialog.fetching = true;
        dialog.fetch_error = None;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { acp_client::registry::fetch_and_cache() })
                .await;
            let updated = view.update(cx, |view, cx| {
                match result {
                    Ok(registry) => {
                        view.registry = Some(Arc::new(registry));
                        if let Some(dialog) = view.add_agent.as_mut() {
                            dialog.fetching = false;
                        }
                    }
                    Err(error) => {
                        if let Some(dialog) = view.add_agent.as_mut() {
                            dialog.fetching = false;
                            dialog.fetch_error = Some(SharedString::from(i18n::t!(
                                "settings.add_agent_fetch_failed",
                                "reason" => format!("{error:#}")
                            )));
                        }
                    }
                }
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("レジストリの取得の結果を画面へ返せない: {error:#}");
            }
        })
        .detach();
    }

    /// 「追加」: `agent_servers.<id>` に `{"type": "registry", "name": …}` を書く。使わないに入って
    /// いたら出す（足したのに一覧で「使わない」のままにしない）。ダイアログは開いたまま（続けて足せる）。
    pub(crate) fn add_registry_agent(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let setting = settings_core::AgentServerSetting::Registry {
            name: Some(name.to_string()),
            env: Default::default(),
        };
        let result = set_agent_server(cx, id, Some(&setting));
        let failed = result.is_err();
        self.report_save(result, cx);
        if !failed && !get(cx).agent_enabled(id) {
            let next = toggled_disabled_agents(&get(cx).disabled_agents, id);
            let result = set_user_value(cx, "disabled_agents", serde_json::json!(next));
            self.report_save(result, cx);
        }
        cx.notify();
    }

    /// 一覧の下の「＋ エージェントを追加」の行（ボタン + 1 行の案内）。
    pub(crate) fn add_agent_row(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .id("add-agent-open")
                    .flex_none()
                    .px(px(10.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(11.5))
                    .text_color(theme.fg1)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(SharedString::from(i18n::t!("settings.agents_add")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, window, cx| view.open_add_agent(window, cx)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.agents_add_hint"))),
            )
    }

    /// 1 件ぶんの行（名前・版・配布の形・説明・対応 OS・ライセンス・このマシンでの注記・右に操作）。
    fn add_agent_entry(
        &self,
        index: usize,
        agent: &acp_client::registry::RegistryAgent,
        registry: &acp_client::registry::Registry,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let builtin = acp_client::custom::builtin_for_registry_id(&agent.id);
        let added = builtin.is_none() && settings.agent_servers.contains_key(&agent.id);
        let probe = acp_client::CustomAgent {
            id: agent.id.clone(),
            label: agent.name.clone(),
            launch: acp_client::CustomLaunch::Registry {
                env: Default::default(),
            },
        };
        let plan = probe.plan(Some(registry));
        let unusable = matches!(plan, Err(acp_client::LaunchError::NoDistribution { .. }));
        let chip = |kind: &str| {
            div()
                .flex_none()
                .px(px(5.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(10.))
                .text_color(theme.fg1)
                .child(SharedString::from(kind.to_string()))
        };
        let mut facts = vec![platform_summary(agent)];
        if let Some(license) = agent
            .license
            .as_deref()
            .filter(|license| !license.is_empty())
        {
            facts.push(i18n::t!("settings.add_agent_license", "license" => license));
        }
        let action: gpui::AnyElement = if let Some(kind) = builtin {
            div()
                .flex_none()
                .text_size(px(11.))
                .text_color(theme.fg2)
                .child(SharedString::from(
                    i18n::t!("settings.add_agent_builtin", "agent" => kind.label),
                ))
                .into_any_element()
        } else if added {
            div()
                .flex_none()
                .text_size(px(11.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("settings.add_agent_added")))
                .into_any_element()
        } else if unusable {
            div()
                .flex_none()
                .text_size(px(11.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("settings.add_agent_unusable")))
                .into_any_element()
        } else {
            let (id, name) = (agent.id.clone(), agent.name.clone());
            div()
                .id(("add-agent", index))
                .flex_none()
                .px(px(10.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(i18n::t!("settings.add_agent_add")))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| {
                        view.add_registry_agent(&id, &name, cx)
                    }),
                )
                .into_any_element()
        };
        div()
            .flex()
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(7.))
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .child(agent_logo(
                None,
                acp_client::custom::monogram_for(&agent.name),
                acp_client::custom::CUSTOM_BRAND_COLOR,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.fg0)
                                    .child(SharedString::from(agent.name.clone())),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(agent.version.clone())),
                            )
                            .children(agent.kinds().into_iter().map(|kind| chip(kind.as_str()))),
                    )
                    .when(!agent.description.is_empty(), |column| {
                        column.child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.fg1)
                                .line_clamp(2)
                                .text_ellipsis()
                                .child(SharedString::from(agent.description.clone())),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(facts.join(" ・ "))),
                    )
                    .when_some(
                        plan_note(&plan).filter(|_| builtin.is_none() && !unusable),
                        |column, (note, notable)| {
                            column.child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(if notable { theme.fg1 } else { theme.fg2 })
                                    .child(SharedString::from(note)),
                            )
                        },
                    ),
            )
            .child(action)
    }

    /// 「エージェントを追加」のダイアログ（設定の面の上・中央・幅 640・高さの上限 640 で中はスクロール）。
    pub(crate) fn render_add_agent(
        &self,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let dialog = self.add_agent.as_ref()?;
        let theme = self.theme.clone();
        let registry = self.registry.clone();
        let blank = dialog.query.is_empty();
        let found: Vec<&acp_client::registry::RegistryAgent> = registry
            .as_deref()
            .map(|registry| registry.search(&dialog.query))
            .unwrap_or_default();
        let link = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_none()
                .text_size(px(11.))
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.fg0))
                .child(SharedString::from(label))
        };
        let header = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(i18n::t!("settings.add_agent_title"))),
                    )
                    .when(registry.is_some(), |row| {
                        row.child(
                            link(
                                "add-agent-refresh",
                                if dialog.fetching {
                                    i18n::t!("settings.add_agent_fetching")
                                } else {
                                    i18n::t!("settings.add_agent_refresh")
                                },
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| view.fetch_registry(cx)),
                            ),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!(
                        "settings.add_agent_sub",
                        "count" => registry.as_ref().map(|registry| registry.agents.len()).unwrap_or(0)
                    ))),
            );
        let search_box = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(8.))
            .rounded(px(6.))
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.))
            .child(div().flex_none().text_color(theme.fg2).child("⌕"))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h(px(18.))
                    .child(dialog.search.clone())
                    .when(blank, |field| {
                        field.child(
                            div()
                                .absolute()
                                .top(px(0.))
                                .left(px(1.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("settings.add_agent_search"))),
                        )
                    }),
            );
        let body: gpui::AnyElement = match registry.as_deref() {
            None => {
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.))
                    .py(px(12.))
                    .child(div().text_size(px(12.)).text_color(theme.fg1).child(
                        SharedString::from(i18n::t!("settings.add_agent_no_registry")),
                    ))
                    .child(
                        div()
                            .id("add-agent-fetch")
                            .px(px(10.))
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(11.5))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                            .child(SharedString::from(if dialog.fetching {
                                i18n::t!("settings.add_agent_fetching")
                            } else {
                                i18n::t!("settings.add_agent_fetch")
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| view.fetch_registry(cx)),
                            ),
                    )
                    .into_any_element()
            }
            Some(_) if found.is_empty() => div()
                .py(px(12.))
                .text_size(px(12.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("settings.add_agent_empty")))
                .into_any_element(),
            Some(registry) => {
                let mut list = div().flex().flex_col().gap(px(6.));
                for (index, agent) in found.iter().enumerate() {
                    list = list.child(self.add_agent_entry(index, agent, registry, settings, cx));
                }
                list.into_any_element()
            }
        };
        let card = div()
            .w(px(640.))
            .max_h(px(640.))
            .flex()
            .flex_col()
            .gap(px(10.))
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
                        view.close_add_agent(cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(search_box)
            .child(
                div()
                    .id("add-agent-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .when_some(dialog.fetch_error.clone(), |card, error| {
                card.child(div().text_size(px(11.)).text_color(theme.err).child(error))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!(
                                "settings.add_agent_custom_hint"
                            ))),
                    )
                    .child(
                        link(
                            "add-agent-settings-json",
                            i18n::t!("settings.add_agent_open_settings_json"),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, _, _window, cx| {
                                view.close_add_agent(cx);
                                cx.emit(SettingsViewEvent::OpenSettingsJson);
                            }),
                        ),
                    )
                    .child(
                        div()
                            .id("add-agent-close")
                            .flex_none()
                            .px(px(13.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.bg1)
                            .text_size(px(12.))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3))
                            .child(SharedString::from(i18n::t!("settings.add_agent_close")))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| view.close_add_agent(cx)),
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
                    cx.listener(|view, _, _window, cx| view.close_add_agent(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> acp_client::registry::Registry {
        acp_client::registry::parse(
            r#"{"version":"1.0.0","agents":[
              {"id":"amp-acp","name":"Amp","version":"0.9.0","description":"",
               "distribution":{"binary":{
                 "darwin-aarch64":{"archive":"https://example.invalid/a","cmd":"./a"},
                 "darwin-x86_64":{"archive":"https://example.invalid/b","cmd":"./a"},
                 "windows-x86_64":{"archive":"https://example.invalid/c","cmd":"a.exe"}}}},
              {"id":"pi-acp","name":"pi ACP","version":"0.0.34","description":"",
               "distribution":{"npx":{"package":"pi-acp@0.0.34"}}}
            ]}"#,
        )
        .expect("見本を読める")
    }

    /// H2-b: binary は検証の値の有無を必ず出す（無い物は注意の色で「検証の値がありません」）。
    #[test]
    fn binaries_say_whether_they_can_be_checked() {
        let verified = Ok(acp_client::LaunchPlan::Binary {
            version: "0.9.0".to_string(),
            deployed: false,
            verified: true,
        });
        assert_eq!(
            plan_note(&verified),
            Some((i18n::t!("settings.add_agent_sha256"), false))
        );
        let unverified = Ok(acp_client::LaunchPlan::Binary {
            version: "0.9.0".to_string(),
            deployed: false,
            verified: false,
        });
        assert_eq!(
            plan_note(&unverified),
            Some((i18n::t!("settings.add_agent_no_sha256"), true))
        );
        assert_eq!(
            plan_note(&Ok(acp_client::LaunchPlan::Npx {
                version: "1".to_string()
            })),
            None
        );
    }

    /// 対応 OS の要約: binary は OS ごとにまとめ、片方の arch だけなら括弧で添える。npx は OS を問わない。
    #[test]
    fn platforms_are_summarized_per_os() {
        let registry = registry();
        let amp = registry.agent("amp-acp").expect("在る");
        let summary = platform_summary(amp);
        assert!(summary.contains("macOS"), "{summary}");
        assert!(
            !summary.contains("macOS（"),
            "両方の arch があれば OS だけ: {summary}"
        );
        assert!(summary.contains("Windows（x64）"), "{summary}");
        assert!(!summary.contains("Linux"), "{summary}");
        let pi = registry.agent("pi-acp").expect("在る");
        assert_eq!(platform_summary(pi), i18n::t!("settings.add_agent_os_any"));
    }
}
