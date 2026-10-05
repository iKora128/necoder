//! 設定 › AI エージェントのページ（issue #38 H2 の見せ方・UI-SPEC §12・見た目の正は
//! `mock/settings-agents.html`）。
//!
//! 組み込み・自分のコマンドで足した物・レジストリから足した物を 1 枚のカードに 1 列で並べ、行ごとに
//! 「このホストで動くか（使える / 足りない物）」「版（今の版とレジストリの版）」「使う / 使わない」を出す。
//! ホスト（このマシン / この窓で開いている SSH のプロジェクトの接続先）を切り替えると、その機械での
//! 状態になる。行を開くと役割（既定・Captain）・起動の上書き（O16）・アカウント（O14）・ログイン・
//! 入れ方がまとまって出る。状態と版は `acp_client::readiness` が読むだけで決める（CLI を起こさない）。
//!
//! 入れない物: CLI 本体の自動更新・他アプリの認証情報に触る機能（zeron の Accounts）。

use super::*;
use acp_client::readiness::{AgentReadiness, HostFacts, Missing, Readiness, VersionInfo};
use acp_client::registry::DistributionKind;
use gpui::ClipboardItem;
use std::collections::BTreeMap;
use std::sync::Arc;

/// ページの行の左端（ロゴ 26 + 間 10 + 行の左 12）。開いた中を名前の左端に揃える。
const DETAILS_INSET: f32 = 48.;

/// AI エージェントのページで選べる SSH 先（workspace が設定を開くたびに渡す）。
#[derive(Clone)]
pub struct AgentHost {
    pub host: Arc<dyn host::Host>,
    /// その接続先で開いているプロジェクトの根（SSH 先のコマンドはプロジェクトの中で流す）。
    pub root: PathBuf,
}

/// SSH 先の事実の読み具合。
pub(crate) enum RemoteFacts {
    /// 読んでいる（世代。古い読みの結果で新しい読みを上書きしない）。
    Reading(u64),
    Ready(HostFacts),
    Failed(SharedString),
}

/// 行に出す見立て。
enum Assessment {
    /// 使わない（`disabled_agents`）。
    Disabled,
    /// まだ読めていない（このマシンのログインの確かめ中・SSH 先を読んでいる）。
    Checking,
    /// SSH 先を読めなかった（理由はホストの行の下に 1 回だけ出す）。
    Failed,
    Known(AgentReadiness),
}

/// 「使わない」にできない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisableRefusal {
    /// 既定のエージェント・Captain（先に別のエージェントを既定にする）。
    DefaultOrCaptain,
    /// 使うエージェントの最後の 1 つ（composer で選べる物が無くなる）。
    Last,
}

impl DisableRefusal {
    pub(crate) fn message(self) -> String {
        match self {
            DisableRefusal::DefaultOrCaptain => i18n::t!("settings.agent_disable_locked"),
            DisableRefusal::Last => i18n::t!("settings.agent_disable_last"),
        }
    }
}

/// `agent_id` を「使わない」にできない理由（できる・もう使わないなら `None`）。外す（足したエージェントを
/// 一覧から消す）も同じ理由で断る。
pub(crate) fn disable_refusal(
    settings: &Settings,
    catalog: &acp_client::AgentCatalog,
    agent_id: &str,
) -> Option<DisableRefusal> {
    if !settings.agent_enabled(agent_id) {
        return None;
    }
    let names = |label: Option<&str>| {
        label
            .and_then(|label| catalog.by_label(label))
            .is_some_and(|agent| agent.id() == agent_id)
    };
    if names(Some(&settings.default_agent)) || names(settings.captain_agent.as_deref()) {
        return Some(DisableRefusal::DefaultOrCaptain);
    }
    let others = catalog
        .agents()
        .iter()
        .filter(|agent| agent.id() != agent_id && settings.agent_enabled(agent.id()))
        .count();
    (others == 0).then_some(DisableRefusal::Last)
}

/// 組み込みの起動の上書き（settings.json の `agent_servers.<組み込みの id>`）を acp_client の言葉へ写す。
pub(crate) fn builtin_overrides(
    settings: &Settings,
) -> BTreeMap<String, acp_client::AgentOverride> {
    settings
        .agent_servers
        .iter()
        .filter(|(id, _)| {
            acp_client::AGENTS
                .iter()
                .any(|agent| agent.id == id.as_str())
        })
        .map(|(id, setting)| {
            let own = match setting {
                settings_core::AgentServerSetting::Custom {
                    command, args, env, ..
                } => acp_client::AgentOverride {
                    command: Some(command.clone()),
                    args: args.clone(),
                    env: env.clone(),
                },
                settings_core::AgentServerSetting::Registry { env, .. } => {
                    acp_client::AgentOverride {
                        command: None,
                        args: Vec::new(),
                        env: env.clone(),
                    }
                }
            };
            (id.clone(), own)
        })
        .collect()
}

/// 足りない物の 1 つ。
fn missing_text(missing: &Missing) -> String {
    match missing {
        Missing::Node => i18n::t!("settings.agent_missing_node"),
        Missing::Uv => i18n::t!("settings.agent_missing_uv"),
        Missing::Cli(name) => i18n::t!("settings.agent_missing_cli", "name" => name),
        Missing::Command(command) => {
            i18n::t!("settings.agent_missing_command", "command" => command)
        }
        Missing::Login => i18n::t!("settings.agent_missing_login"),
    }
}

/// 版の行（出す物が無ければ `None`）。
pub(crate) fn version_text(version: &VersionInfo) -> Option<String> {
    version_text_with(version, &i18n::translate)
}

/// [`version_text`] の本体（訳の引き方を渡せる・テストは ja で文を固定する）。
fn version_text_with(version: &VersionInfo, translate: &dyn Fn(&str) -> String) -> Option<String> {
    let fill = |key: &str, pairs: &[(&str, &str)]| {
        pairs.iter().fold(translate(key), |text, (name, value)| {
            text.replace(&format!("%{{{name}}}"), value)
        })
    };
    Some(match version {
        VersionInfo::None => return None,
        VersionInfo::OwnCommand { command } => {
            fill("settings.agent_version_own", &[("command", command)])
        }
        VersionInfo::Registry { version } => {
            fill("settings.agent_version_registry", &[("version", version)])
        }
        VersionInfo::Path {
            command,
            current,
            registry,
        } => match (current, registry) {
            (Some(current), Some(registry)) if current == registry => fill(
                "settings.agent_version_path_same",
                &[("version", current), ("command", command)],
            ),
            (Some(current), Some(registry)) => fill(
                "settings.agent_version_path_behind",
                &[
                    ("current", current),
                    ("registry", registry),
                    ("command", command),
                ],
            ),
            (Some(current), None) => fill(
                "settings.agent_version_path_current",
                &[("version", current), ("command", command)],
            ),
            (None, Some(registry)) => fill(
                "settings.agent_version_path_registry",
                &[("registry", registry), ("command", command)],
            ),
            (None, None) => fill("settings.agent_version_path", &[("command", command)]),
        },
        VersionInfo::Managed {
            kind,
            current,
            target,
            fetchable,
            verified,
        } => {
            let binary = *kind == DistributionKind::Binary;
            match (current, fetchable) {
                (Some(current), _) if current == target => match (binary, verified) {
                    (false, _) => fill("settings.agent_version_same", &[("version", target)]),
                    (true, true) => {
                        fill("settings.agent_version_binary_same", &[("version", target)])
                    }
                    (true, false) => fill(
                        "settings.agent_version_binary_same_unverified",
                        &[("version", target)],
                    ),
                },
                (Some(current), true) => {
                    let key = match (binary, verified) {
                        (false, _) => "settings.agent_version_update",
                        (true, true) => "settings.agent_version_binary_update",
                        (true, false) => "settings.agent_version_binary_update_unverified",
                    };
                    fill(key, &[("current", current), ("target", target)])
                }
                (None, true) => {
                    let key = match (binary, verified) {
                        (false, _) => "settings.agent_version_fetch",
                        (true, true) => "settings.agent_version_binary_fetch",
                        (true, false) => "settings.agent_version_binary_fetch_unverified",
                    };
                    fill(key, &[("version", target)])
                }
                // 取りに行く道具が無い時は「次の起動で」と約束しない。
                (Some(current), false) => fill(
                    "settings.agent_version_behind",
                    &[("current", current), ("target", target)],
                ),
                (None, false) => fill("settings.agent_version_registry", &[("version", target)]),
            }
        }
    })
}

/// 種類の文字（名前の右・fg2）。
fn kind_text(
    agent: &acp_client::Agent,
    registry: Option<&acp_client::registry::Registry>,
) -> String {
    match agent {
        acp_client::Agent::Builtin(_) => i18n::t!("settings.agent_kind_builtin"),
        acp_client::Agent::Custom(custom) => match &custom.launch {
            acp_client::CustomLaunch::Command { .. } => i18n::t!("settings.agent_kind_command"),
            acp_client::CustomLaunch::Registry { .. } => {
                let kind = registry
                    .and_then(|registry| registry.agent(&custom.id))
                    .and_then(|entry| {
                        entry
                            .launch()
                            .map(|launch| launch.kind())
                            .or_else(|| entry.kinds().first().copied())
                    });
                match kind {
                    Some(kind) => {
                        i18n::t!("settings.agent_kind_registry_with", "kind" => kind.as_str())
                    }
                    None => i18n::t!("settings.agent_kind_registry"),
                }
            }
        },
    }
}

impl SettingsView {
    // ── ホスト（このマシン / SSH 先）──────────────────────────────────────────────

    /// 選べる SSH 先を受け取る（同じ接続先は 1 つ・渡した順）。見ていた SSH 先が無くなったらこのマシンへ戻す。
    pub fn set_agent_hosts(&mut self, hosts: Vec<AgentHost>) {
        let mut unique: Vec<AgentHost> = Vec::new();
        for host in hosts {
            if host.host.is_remote()
                && !unique.iter().any(|known| known.host.id() == host.host.id())
            {
                unique.push(host);
            }
        }
        self.agent_hosts = unique;
        let known = |id: &str| self.agent_hosts.iter().any(|host| host.host.id() == id);
        if self.agent_host.as_ref().is_some_and(|id| !known(id)) {
            self.agent_host = None;
        }
        let hosts = &self.agent_hosts;
        self.remote_facts
            .retain(|id, _| hosts.iter().any(|host| host.host.id() == id.as_ref()));
    }

    /// ホストのチップを押した（`None` = このマシン）。SSH 先はまだ読んでいなければ読む。
    pub(crate) fn select_agent_host(&mut self, host: Option<SharedString>, cx: &mut Context<Self>) {
        if self.agent_host == host {
            return;
        }
        self.agent_host = host.clone();
        if let Some(id) = host {
            let read = matches!(
                self.remote_facts.get(&id),
                Some(RemoteFacts::Ready(_) | RemoteFacts::Reading(_))
            );
            if !read {
                self.probe_agent_host(id, cx);
            }
        }
        cx.notify();
    }

    /// 「確かめ直す」: 見ているホストを読み直す。
    fn recheck_agent_host(&mut self, cx: &mut Context<Self>) {
        match self.agent_host.clone() {
            Some(id) => self.probe_agent_host(id, cx),
            None => self.refresh_availability(cx),
        }
        cx.notify();
    }

    /// SSH 先を読む（背景・その接続先で開いているプロジェクトの接続で、読み取りだけのシェルを 1 回）。
    pub(crate) fn probe_agent_host(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let Some(target) = self
            .agent_hosts
            .iter()
            .find(|host| host.host.id() == id.as_ref())
            .cloned()
        else {
            return;
        };
        self.agent_host_generation = self.agent_host_generation.wrapping_add(1);
        let generation = self.agent_host_generation;
        self.remote_facts
            .insert(id.clone(), RemoteFacts::Reading(generation));
        let agents = agent_catalog(cx).agents();
        let overrides = builtin_overrides(&get(cx));
        let registry = self.registry.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    acp_client::readiness::probe_remote(
                        target.host.as_ref(),
                        &target.root,
                        &agents,
                        &overrides,
                        registry.as_deref(),
                    )
                })
                .await;
            let applied = view.update(cx, |view, cx| {
                let current = matches!(
                    view.remote_facts.get(&id),
                    Some(RemoteFacts::Reading(reading)) if *reading == generation
                );
                if !current {
                    return;
                }
                view.remote_facts.insert(
                    id,
                    match result {
                        Ok(facts) => RemoteFacts::Ready(facts),
                        Err(error) => RemoteFacts::Failed(SharedString::from(format!("{error:#}"))),
                    },
                );
                cx.notify();
            });
            if let Err(error) = applied {
                eprintln!("SSH 先のエージェントの状態を画面へ返せない: {error:#}");
            }
        })
        .detach();
    }

    /// 設定を開き直した時: 見ている SSH 先を読み直す（このマシンは [`Self::refresh_availability`] が読む）。
    pub(crate) fn refresh_agent_host(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.agent_host.clone() {
            self.probe_agent_host(id, cx);
        }
    }

    /// ホストの行: `ホスト` + チップ（このマシン + SSH 先）+ 右に「確かめ直す」、下に注記（と読めなかった理由）。
    pub(crate) fn agent_host_row(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let chip = |id: (&'static str, usize), selected: bool| {
            div()
                .id(id)
                .flex_none()
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(5.))
                .text_size(px(11.))
                .whitespace_nowrap()
                .cursor_pointer()
                .when(selected, |chip| chip.bg(theme.bg3).text_color(theme.fg0))
                .when(!selected, |chip| {
                    chip.border_1()
                        .border_color(theme.border)
                        .text_color(theme.fg1)
                        .hover(|style| style.text_color(theme.fg0))
                })
        };
        let mut chips = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .flex_none()
                    .w(px(56.))
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.agent_host_label"))),
            )
            .child(
                chip(("agent-host", 0), self.agent_host.is_none())
                    .child(SharedString::from(i18n::t!("settings.agent_host_local")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _window, cx| view.select_agent_host(None, cx)),
                    ),
            );
        for (position, target) in self.agent_hosts.iter().enumerate() {
            let id = SharedString::from(target.host.id().to_string());
            let selected = self.agent_host.as_ref() == Some(&id);
            chips = chips.child(
                chip(("agent-host", position + 1), selected)
                    .child(SharedString::from(target.host.display_name().to_string()))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("settings.agent_host_ssh"))),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.select_agent_host(Some(id.clone()), cx)
                        }),
                    ),
            );
        }
        chips = chips.child(div().flex_1()).child(
            div()
                .id("agent-host-recheck")
                .flex_none()
                .text_size(px(11.))
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.fg0))
                .child(SharedString::from(i18n::t!("settings.agent_host_recheck")))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, _, _window, cx| view.recheck_agent_host(cx)),
                ),
        );
        let failure = self.agent_host.as_ref().and_then(|id| {
            let RemoteFacts::Failed(reason) = self.remote_facts.get(id)? else {
                return None;
            };
            let name = self
                .agent_hosts
                .iter()
                .find(|host| host.host.id() == id.as_ref())
                .map(|host| host.host.display_name().to_string())
                .unwrap_or_else(|| id.to_string());
            Some(i18n::t!("settings.agent_host_failed", "host" => name, "reason" => reason))
        });
        let note = |text: String, color: Hsla| {
            div()
                .pl(px(60.))
                .text_size(px(10.5))
                .text_color(color)
                .child(SharedString::from(text))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(chips)
            .child(note(i18n::t!("settings.agent_host_note"), theme.fg2))
            .when_some(failure, |row, failure| row.child(note(failure, theme.fg1)))
    }

    // ── 一覧 ─────────────────────────────────────────────────────────────────────

    /// このマシンの見立て（ログインの確かめ中・まだ読めていなければ `None`）。
    fn local_readiness(
        &self,
        agent: &acp_client::Agent,
        overrides: &BTreeMap<String, acp_client::AgentOverride>,
    ) -> Option<AgentReadiness> {
        if self.checking_agents {
            return None;
        }
        let facts = self.local_facts.as_ref()?;
        let login = agent.builtin().and_then(|kind| {
            let index = acp_client::AGENTS
                .iter()
                .position(|known| known.id == kind.id)?;
            self.auth_states.get(index).copied()
        });
        Some(acp_client::readiness::assess(
            agent,
            facts,
            login,
            overrides.get(agent.id()),
            self.registry.as_deref(),
        ))
    }

    /// 見ているホストでの見立て。
    fn assessment(
        &self,
        agent: &acp_client::Agent,
        enabled: bool,
        local: Option<&AgentReadiness>,
        overrides: &BTreeMap<String, acp_client::AgentOverride>,
        remote: Option<&SharedString>,
    ) -> Assessment {
        if !enabled {
            return Assessment::Disabled;
        }
        let Some(host) = remote else {
            return local
                .cloned()
                .map_or(Assessment::Checking, Assessment::Known);
        };
        match self.remote_facts.get(host) {
            Some(RemoteFacts::Ready(facts)) => Assessment::Known(acp_client::readiness::assess(
                agent,
                facts,
                None,
                overrides.get(agent.id()),
                self.registry.as_deref(),
            )),
            Some(RemoteFacts::Failed(_)) => Assessment::Failed,
            Some(RemoteFacts::Reading(_)) | None => Assessment::Checking,
        }
    }

    /// エージェント一覧（1 枚のカードに 1 列）。`page` = 設定のページ（行を開ける・役割・ホストの切り替え）/
    /// `false` = 初回の案内（行の頭だけ・このマシンだけ・UI-SPEC §12）。
    pub(crate) fn agents_list(
        &self,
        settings: &Settings,
        page: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let catalog = agent_catalog(cx);
        let overrides = builtin_overrides(settings);
        let remote = if page { self.agent_host.clone() } else { None };
        let mut list = div()
            .flex()
            .flex_col()
            .rounded(px(9.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border);
        for (index, agent) in catalog.agents().iter().enumerate() {
            let row = self.agent_row(
                index,
                agent,
                settings,
                &catalog,
                &overrides,
                remote.as_ref(),
                page,
                cx,
            );
            list = list.child(
                div()
                    .when(index > 0, |row| row.border_t_1().border_color(theme.border))
                    .child(row),
            );
        }
        list
    }

    /// 1 行（頭 + 開いていれば中）。
    #[allow(clippy::too_many_arguments)]
    fn agent_row(
        &self,
        index: usize,
        agent: &acp_client::Agent,
        settings: &Settings,
        catalog: &acp_client::AgentCatalog,
        overrides: &BTreeMap<String, acp_client::AgentOverride>,
        remote: Option<&SharedString>,
        page: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let id = SharedString::from(agent.id().to_string());
        let enabled = settings.agent_enabled(agent.id());
        let is_default = settings.default_agent == agent.label();
        let is_captain = settings.captain_agent.as_deref() == Some(agent.label());
        let local = self.local_readiness(agent, overrides);
        let assessment = self.assessment(agent, enabled, local.as_ref(), overrides, remote);
        let expanded = page && self.expanded_agent.as_ref() == Some(&id);
        let locked = disable_refusal(settings, catalog, agent.id()).is_some();
        let version = match &assessment {
            Assessment::Known(found) => version_text(&found.version),
            _ => None,
        };
        let badge = |text: String, accent: bool| {
            div()
                .flex_none()
                .px(px(6.))
                .rounded(px(4.))
                .text_size(px(10.5))
                .when(accent, |badge| {
                    badge.bg(self.accent.alpha(0.16)).text_color(self.accent)
                })
                .when(!accent, |badge| badge.bg(theme.bg3).text_color(theme.fg1))
                .child(SharedString::from(text))
        };
        let (icon, monogram, brand) = agent.brand();
        let text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(agent.label().to_string())),
                    )
                    .child(div().text_size(px(10.5)).text_color(theme.fg2).child(
                        SharedString::from(kind_text(agent, self.registry.as_deref())),
                    ))
                    .when(is_default, |line| {
                        line.child(badge(i18n::t!("settings.is_default"), true))
                    })
                    .when(is_captain && page, |line| {
                        line.child(badge(i18n::t!("settings.is_captain"), false))
                    }),
            )
            .child(self.status_line(&assessment, remote.is_some()))
            .when_some(version, |text, version| {
                text.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(version)),
                )
            });
        let action = if remote.is_none() {
            self.header_action(index, agent, &assessment, cx)
        } else {
            None
        };
        let agent_id = id.clone();
        let switch = self
            .switch(("agent-enabled", index), enabled)
            .flex_none()
            .when(locked, |switch| switch.opacity(0.45).cursor_default())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _window, cx| {
                    cx.stop_propagation();
                    view.toggle_agent_enabled(&agent_id, cx);
                }),
            );
        let toggle_id = id.clone();
        let header = div()
            .id(("agent-row", index))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(10.))
            .when(page, |header| {
                header.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| {
                        view.toggle_agent_details(&toggle_id, cx)
                    }),
                )
            })
            .child(
                div()
                    .flex_none()
                    .when(!enabled, |logo| logo.opacity(0.4))
                    .child(agent_logo(icon, monogram, brand)),
            )
            .child(text)
            .children(action)
            .when(page, |header| {
                header.child(
                    div()
                        .flex_none()
                        .w(px(14.))
                        .flex()
                        .justify_center()
                        .text_size(px(13.))
                        .text_color(theme.fg2)
                        .child(if expanded { "⌄" } else { "›" }),
                )
            })
            .child(switch);
        div().flex().flex_col().child(header).when(expanded, |row| {
            row.child(self.agent_details(
                index,
                agent,
                settings,
                enabled,
                is_default,
                is_captain,
                local.as_ref(),
                remote.is_some(),
                cx,
            ))
        })
    }

    /// 行を開く / 閉じる（1 つだけ開く・ビューのメモリ）。
    pub(crate) fn toggle_agent_details(&mut self, agent_id: &SharedString, cx: &mut Context<Self>) {
        self.expanded_agent = if self.expanded_agent.as_ref() == Some(agent_id) {
            None
        } else {
            Some(agent_id.clone())
        };
        cx.notify();
    }

    /// 状態の文（色相を使わない・文字の濃さで分ける）。
    fn status_line(&self, assessment: &Assessment, remote: bool) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let line = |color: Hsla, text: String| {
            div()
                .text_size(px(11.))
                .text_color(color)
                .child(SharedString::from(text))
                .into_any_element()
        };
        let found = match assessment {
            Assessment::Disabled => {
                return line(theme.fg2, i18n::t!("settings.agent_status_disabled"))
            }
            Assessment::Checking => {
                return line(theme.fg2, i18n::t!("settings.agent_status_checking"))
            }
            Assessment::Failed => {
                return line(theme.fg0, i18n::t!("settings.agent_status_unreachable"))
            }
            Assessment::Known(found) => found,
        };
        match &found.readiness {
            Readiness::Ready => line(theme.fg1, i18n::t!("settings.agent_status_ready")),
            Readiness::Unverified => line(
                theme.fg0,
                if remote {
                    i18n::t!("settings.agent_status_unverified_remote")
                } else {
                    i18n::t!("settings.agent_status_unverified")
                },
            ),
            Readiness::Missing(items) => div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .text_size(px(11.))
                .child(
                    div()
                        .text_color(theme.fg1)
                        .child(SharedString::from(i18n::t!(
                            "settings.agent_status_missing"
                        ))),
                )
                .child(
                    div().text_color(theme.fg0).child(SharedString::from(
                        items
                            .iter()
                            .map(missing_text)
                            .collect::<Vec<_>>()
                            .join(&i18n::t!("settings.agent_missing_separator")),
                    )),
                )
                .into_any_element(),
            Readiness::Unavailable(error) => line(theme.fg0, add_agent::launch_problem_text(error)),
        }
    }

    /// 行の頭の右の枠のボタン（このマシン・使う組み込みだけ）: 入れる / ログイン / CLI を開く。
    fn header_action(
        &self,
        index: usize,
        agent: &acp_client::Agent,
        assessment: &Assessment,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let kind = agent.builtin()?;
        let Assessment::Known(found) = assessment else {
            return None;
        };
        let (text, command) = match &found.readiness {
            Readiness::Missing(items)
                if items.iter().any(|item| matches!(item, Missing::Cli(_)))
                    && !items
                        .iter()
                        .any(|item| matches!(item, Missing::Node | Missing::Uv)) =>
            {
                (i18n::t!("settings.agent_action_install"), kind.install_cmd)
            }
            Readiness::Missing(items) if items.as_slice() == [Missing::Login] => {
                (i18n::t!("settings.agent_action_login"), kind.login_cmd)
            }
            Readiness::Unverified => (i18n::t!("settings.agent_action_open_cli"), kind.login_cmd),
            _ => return None,
        };
        let theme = self.theme.clone();
        let agent_index = acp_client::AGENTS
            .iter()
            .position(|known| known.id == kind.id)?;
        Some(
            div()
                .id(("agent-action", index))
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
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(text))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| {
                        cx.stop_propagation();
                        view.run_agent_command(agent_index, command, cx);
                    }),
                )
                .into_any_element(),
        )
    }

    /// 導入 / ログインのコマンドをターミナルで流す。コマンドの終わりは取れない（ターミナルはシェルに
    /// 落ちる）ので、押した時点から変化を見張って反映の遅れを消す。
    fn run_agent_command(
        &mut self,
        agent_index: usize,
        command: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.watch_agent_progress(agent_index, cx);
        cx.emit(SettingsViewEvent::RunCommand(command.to_string()));
    }

    /// コマンドをクリップボードへ（SSH 先で打つ時など）。
    fn copy_agent_command(&mut self, command: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
        cx.emit(SettingsViewEvent::Notice(SharedString::from(i18n::t!(
            "settings.agent_copied",
            "command" => command
        ))));
    }

    // ── 開いた中 ─────────────────────────────────────────────────────────────────

    /// 開いた行の中（名前の左端に揃えて行の続きとして出す・箱を作らない）。
    #[allow(clippy::too_many_arguments)]
    fn agent_details(
        &self,
        index: usize,
        agent: &acp_client::Agent,
        settings: &Settings,
        enabled: bool,
        is_default: bool,
        is_captain: bool,
        local: Option<&AgentReadiness>,
        remote: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let id = SharedString::from(agent.id().to_string());
        let label = SharedString::from(agent.label().to_string());
        let mut details = div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .pl(px(DETAILS_INSET))
            .pr(px(12.))
            .pb(px(12.));
        // 既定・Captain にできるのは、このマシンで使える時だけ（今までの「既定にする」と同じ）。
        let usable = local.is_some_and(|found| found.readiness == Readiness::Ready);
        if enabled {
            details = details.child(
                self.detail_line(
                    i18n::t!("settings.agent_detail_role"),
                    self.role_controls(
                        index,
                        label.clone(),
                        is_default,
                        is_captain,
                        usable,
                        settings,
                        cx,
                    )
                    .into_any_element(),
                ),
            );
        }
        let custom = agent.custom();
        details = details.child(
            self.detail_line(
                i18n::t!("settings.agent_detail_launch"),
                self.launch_controls(
                    index,
                    id.clone(),
                    label.clone(),
                    custom.is_some(),
                    settings,
                    cx,
                )
                .into_any_element(),
            ),
        );
        if let Some(text) = custom.and_then(|custom| self.distribution_text(custom, remote)) {
            details = details.child(self.detail_line(
                i18n::t!("settings.agent_detail_distribution"),
                div().child(SharedString::from(text)).into_any_element(),
            ));
        }
        if let Some(kind) = agent.builtin() {
            let cli_present = self
                .local_facts
                .as_ref()
                .is_some_and(|facts| facts.has_command(kind.cli_command()));
            if let Some(var) = settings_core::account_env_var(kind.id)
                .filter(|_| !remote && enabled && cli_present && !cfg!(windows))
            {
                if let Some(agent_index) = acp_client::AGENTS
                    .iter()
                    .position(|known| known.id == kind.id)
                {
                    details = details.child(
                        self.detail_line(
                            i18n::t!("settings.agent_detail_account"),
                            self.account_controls(agent_index, kind.id, var, settings, cx)
                                .into_any_element(),
                        ),
                    );
                }
            }
            let agent_index = acp_client::AGENTS
                .iter()
                .position(|known| known.id == kind.id)
                .unwrap_or(index);
            details = details
                .child(
                    self.detail_line(
                        i18n::t!("settings.agent_detail_login"),
                        self.command_controls(
                            ("agent-login", index),
                            kind.login_cmd,
                            (!remote).then(|| {
                                (i18n::t!("settings.agent_detail_run_login"), agent_index)
                            }),
                            cx,
                        )
                        .into_any_element(),
                    ),
                )
                .child(
                    self.detail_line(
                        i18n::t!("settings.agent_detail_install"),
                        self.command_controls(
                            ("agent-install", index),
                            kind.install_cmd,
                            (!remote).then(|| {
                                (i18n::t!("settings.agent_detail_run_install"), agent_index)
                            }),
                            cx,
                        )
                        .into_any_element(),
                    ),
                );
            if remote {
                details = details.child(
                    div()
                        .pl(px(80.))
                        .text_size(px(10.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!(
                            "settings.agent_detail_remote_note"
                        ))),
                );
            }
        }
        details
    }

    /// 開いた中の 1 行（ラベル 70 + 中身）。
    fn detail_line(&self, label: String, content: gpui::AnyElement) -> Div {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_start()
            .gap(px(10.))
            .child(
                div()
                    .flex_none()
                    .w(px(70.))
                    .pt(px(2.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(label)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(theme.fg1)
                    .child(content),
            )
    }

    /// 文字のリンク（fg2・hover fg0）。
    fn detail_link(&self, id: (&'static str, usize), text: String) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .flex_none()
            .whitespace_nowrap()
            .text_color(theme.fg2)
            .cursor_pointer()
            .hover(|style| style.text_color(theme.fg0))
            .child(SharedString::from(text))
    }

    /// 枠のボタン（選択 = bg3 + fg0・識別色は使わない）。
    fn detail_button(
        &self,
        id: (&'static str, usize),
        text: String,
        chosen: bool,
    ) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .flex_none()
            .h(px(20.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .whitespace_nowrap()
            .when(chosen, |button| button.bg(theme.bg3).text_color(theme.fg0))
            .when(!chosen, |button| {
                button
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.fg1)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
            })
            .child(SharedString::from(text))
    }

    /// 役割: `★ 既定` / `既定にする` と `⚑ Captain（押すとやめる）` / `Captain にする`。
    #[allow(clippy::too_many_arguments)]
    fn role_controls(
        &self,
        index: usize,
        label: SharedString,
        is_default: bool,
        is_captain: bool,
        usable: bool,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut controls = div().flex().flex_wrap().items_center().gap(px(6.));
        if is_default {
            controls = controls.child(self.detail_button(
                ("agent-default", index),
                i18n::t!("settings.is_default"),
                true,
            ));
        } else if usable {
            let label = label.clone();
            controls = controls.child(
                self.detail_button(
                    ("agent-default", index),
                    i18n::t!("settings.make_default"),
                    false,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| view.set_default_agent(&label, cx)),
                ),
            );
        }
        if is_captain || usable {
            let current = settings.captain_agent.clone();
            controls = controls.child(
                self.detail_button(
                    ("agent-captain", index),
                    if is_captain {
                        i18n::t!("settings.captain_dismiss")
                    } else {
                        i18n::t!("settings.make_captain")
                    },
                    is_captain,
                )
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| {
                        view.toggle_captain(&label, current.as_deref(), cx)
                    }),
                ),
            );
        }
        if !is_default && !usable {
            controls = controls.child(
                div()
                    .text_color(self.theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.agent_role_unusable"))),
            );
        }
        controls
    }

    /// 配布（レジストリから足した物）: `npx · pi-acp@0.0.34` / `uvx · …` / `binary · darwin-aarch64 · sha256 あり`。
    fn distribution_text(&self, custom: &acp_client::CustomAgent, remote: bool) -> Option<String> {
        if !matches!(custom.launch, acp_client::CustomLaunch::Registry { .. }) {
            return None;
        }
        let entry = self.registry.as_deref()?.agent(&custom.id)?;
        let platform = if remote {
            None
        } else {
            acp_client::registry::platform_key()
        };
        Some(match entry.launch_for(platform)? {
            acp_client::registry::Launch::Npx(npx) => {
                i18n::t!("settings.agent_distribution_npx", "package" => npx.package)
            }
            acp_client::registry::Launch::Uvx(uvx) => {
                i18n::t!("settings.agent_distribution_uvx", "package" => uvx.package)
            }
            acp_client::registry::Launch::Binary {
                platform,
                distribution,
            } => {
                if distribution.sha256.is_some() {
                    i18n::t!("settings.agent_distribution_binary", "platform" => platform)
                } else {
                    i18n::t!(
                        "settings.agent_distribution_binary_unverified",
                        "platform" => platform
                    )
                }
            }
        })
    }

    /// コマンド（等幅・長ければ 1 行で省略）+ `ターミナルで…`（このマシンだけ）+ `コピー`。
    fn command_controls(
        &self,
        id: (&'static str, usize),
        command: &'static str,
        run: Option<(String, usize)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let index = id.1;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .min_w_0()
                    .max_w_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(theme.bg1)
                    .border_1()
                    .border_color(theme.border)
                    .font_family(ui::code_font(cx))
                    .text_size(px(11.))
                    .text_color(theme.fg0)
                    .child(SharedString::new_static(command)),
            )
            .when_some(run, |controls, (text, agent_index)| {
                controls.child(
                    self.detail_link((id.0, index * 10 + 1), text)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, _window, cx| {
                                view.run_agent_command(agent_index, command, cx)
                            }),
                        ),
                )
            })
            .child(
                self.detail_link(
                    (id.0, index * 10 + 2),
                    i18n::t!("settings.agent_detail_copy"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| view.copy_agent_command(command, cx)),
                ),
            )
    }

    /// アカウント（O14・Claude Code / Codex）: 既定 / 作ったアカウント / ＋ 新しいアカウント / ログイン。
    /// 選ぶと `agent_servers.<id>.env.<var>` を書く（資格情報は読まない・置き場を指すだけ）。
    fn account_controls(
        &self,
        index: usize,
        agent_id: &'static str,
        var: &'static str,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let current = settings
            .agent_servers
            .get(agent_id)
            .and_then(|server| server.env().get(var))
            .cloned();
        let root = settings_core::accounts_root(agent_id);
        let accounts = self.accounts.get(index).cloned().unwrap_or_default();
        let selected = current.as_deref().and_then(|path| {
            let root = root.as_ref()?;
            let name = Path::new(path)
                .strip_prefix(root)
                .ok()?
                .to_str()?
                .to_string();
            accounts.contains(&name).then_some(name)
        });
        let mut line = div().flex().flex_wrap().items_center().gap(px(4.)).child(
            self.detail_button(
                ("account-default", index),
                i18n::t!("settings.account_default"),
                current.is_none(),
            )
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _window, cx| view.use_account(agent_id, None, cx)),
            ),
        );
        for (position, name) in accounts.iter().enumerate() {
            let chosen = selected.as_deref() == Some(name.as_str());
            let account = name.clone();
            line = line.child(
                self.detail_button(("account", index * 100 + position), name.clone(), chosen)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.use_account(agent_id, Some(account.clone()), cx)
                        }),
                    ),
            );
        }
        // settings.json で necoder の置き場の外を指している（手で書いた）: 選ばれていることだけ見せる。
        if current.is_some() && selected.is_none() {
            line = line.child(self.detail_button(
                ("account-elsewhere", index),
                i18n::t!("settings.account_elsewhere"),
                true,
            ));
        }
        line = line.child(
            self.detail_button(
                ("account-add", index),
                i18n::t!("settings.account_add"),
                false,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _window, cx| view.add_account(index, cx)),
            ),
        );
        if let Some(path) = current {
            line = line.child(
                self.detail_link(("account-login", index), i18n::t!("settings.account_login"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.login_account(index, Path::new(&path), cx)
                        }),
                    ),
            );
        }
        div().flex().flex_col().gap(px(3.)).child(line).child(
            div()
                .text_size(px(10.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("settings.account_hint"))),
        )
    }

    // ── 開発用（隔離 offscreen の撮影）────────────────────────────────────────────

    /// 開発用: AI エージェントのページで行を開く（`agent_id` が空なら閉じる）。
    #[cfg(debug_assertions)]
    pub fn debug_expand_agent(&mut self, agent_id: &str, cx: &mut Context<Self>) {
        self.select_page(SettingsPage::Agents, cx);
        self.expanded_agent =
            (!agent_id.is_empty()).then(|| SharedString::from(agent_id.to_string()));
        cx.notify();
    }

    /// 開発用: 偽の SSH 先（一時フォルダの偽のホームで、本物と同じ読み取りのシェルを流す）を足して選ぶ。
    /// SSH 先が無い環境で「SSH 先へ切り替えた所」を撮るため。本物の SSH には繋がない。
    #[cfg(debug_assertions)]
    pub fn debug_use_fake_host(&mut self, name: &str, cx: &mut Context<Self>) {
        let fake = match fake_ssh::FakeSshHost::create(name) {
            Ok(fake) => fake,
            Err(error) => {
                eprintln!("偽の SSH 先を作れない: {error:#}");
                return;
            }
        };
        let root = fake.root();
        let id = SharedString::from(host::Host::id(&fake).to_string());
        let mut hosts = self.agent_hosts.clone();
        hosts.push(AgentHost {
            host: Arc::new(fake),
            root,
        });
        self.set_agent_hosts(hosts);
        self.select_page(SettingsPage::Agents, cx);
        self.select_agent_host(Some(id), cx);
    }
}

/// 開発用の偽の SSH 先（debug ビルドだけ）。一時フォルダに偽のホーム（node・npx・PATH の
/// claude-agent-acp・codex・opencode・dsh-acp と、npx のキャッシュ・ログインの跡）を作り、
/// SSH 先へ流すのと同じシェルを `sh -c` でそこへ流す（環境は空にして HOME と PATH だけ渡す）。
#[cfg(debug_assertions)]
mod fake_ssh {
    use anyhow::{Context as _, Result};
    use host::{CommandOutput, CommandSpec, Host};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    pub(super) struct FakeSshHost {
        id: String,
        display_name: String,
        home: PathBuf,
        inner: Arc<dyn Host>,
    }

    impl FakeSshHost {
        pub(super) fn create(name: &str) -> Result<Self> {
            let name = if name.is_empty() {
                "fake@dev-box"
            } else {
                name
            };
            let home = std::env::temp_dir().join(format!(
                "necoder-fake-ssh-{}-{}",
                std::process::id(),
                name.replace(['@', '/', ' '], "-")
            ));
            build_home(&home).context("偽のホームを作れない")?;
            Ok(Self {
                id: format!("ssh://{name}"),
                display_name: name.to_string(),
                home,
                inner: host::LocalHost::shared(),
            })
        }

        pub(super) fn root(&self) -> PathBuf {
            self.home.clone()
        }

        fn answer(&self, spec: &CommandSpec) -> Result<CommandOutput> {
            // SSH 先へは `sh -lc <script>` で流す。偽のホームではログインシェルの設定を読まない。
            let script = spec.args.last().cloned().unwrap_or_default();
            let output = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .env_clear()
                .env("HOME", &self.home)
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", self.home.join("bin").display()),
                )
                .env("OPENAI_API_KEY", "fake")
                .output()
                .context("偽の SSH 先でシェルを流せない")?;
            Ok(CommandOutput {
                status_code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
    }

    fn executable(path: &Path) -> Result<()> {
        std::fs::write(path, "#!/bin/sh\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }

    /// 偽のホーム: claude（ログインの跡と PATH のアダプタ 0.70.2）・codex（API キーの環境変数）・
    /// opencode（跡なし）・dsh-acp・npx のキャッシュの pi-acp 0.0.30。
    fn build_home(home: &Path) -> Result<()> {
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin)?;
        for name in ["node", "npx", "claude", "codex", "opencode", "dsh-acp"] {
            executable(&bin.join(name))?;
        }
        let adapter = home.join("lib/node_modules/@agentclientprotocol/claude-agent-acp");
        std::fs::create_dir_all(adapter.join("dist"))?;
        std::fs::write(
            adapter.join("package.json"),
            "{\n  \"name\": \"@agentclientprotocol/claude-agent-acp\",\n  \"version\": \"0.70.2\"\n}\n",
        )?;
        executable(&adapter.join("dist/index.js"))?;
        let link = bin.join("claude-agent-acp");
        if !link.exists() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(adapter.join("dist/index.js"), &link)?;
        }
        let cached = home.join(".npm/_npx/f00d/node_modules/pi-acp");
        std::fs::create_dir_all(&cached)?;
        std::fs::write(
            cached.join("package.json"),
            "{\n  \"name\": \"pi-acp\",\n  \"version\": \"0.0.30\"\n}\n",
        )?;
        std::fs::create_dir_all(home.join(".claude"))?;
        std::fs::write(home.join(".claude/.credentials.json"), "{}")?;
        Ok(())
    }

    impl Host for FakeSshHost {
        fn id(&self) -> &str {
            &self.id
        }
        fn display_name(&self) -> &str {
            &self.display_name
        }
        fn is_remote(&self) -> bool {
            true
        }
        fn host_for_project(&self, path: &Path) -> Result<Arc<dyn Host>> {
            self.inner.host_for_project(path)
        }
        fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
            self.inner.canonicalize(path)
        }
        fn metadata(&self, path: &Path) -> Result<host::HostMetadata> {
            self.inner.metadata(path)
        }
        fn read_dir(&self, path: &Path) -> Result<Vec<host::HostEntry>> {
            self.inner.read_dir(path)
        }
        fn read_file(&self, path: &Path) -> Result<host::FileContent> {
            self.inner.read_file(path)
        }
        fn write_file(
            &self,
            path: &Path,
            bytes: &[u8],
            condition: host::WriteCondition,
        ) -> Result<host::FileRevision> {
            self.inner.write_file(path, bytes, condition)
        }
        fn list_files(&self, root: &Path, limit: usize) -> Result<Vec<PathBuf>> {
            self.inner.list_files(root, limit)
        }
        fn search_project(
            &self,
            root: &Path,
            spec: &host::TextSearchSpec,
            file_limit: usize,
        ) -> Result<Vec<host::TextSearchHit>> {
            self.inner.search_project(root, spec, file_limit)
        }
        fn run_command(&self, spec: &CommandSpec) -> Result<CommandOutput> {
            self.answer(spec)
        }
        fn run_command_retry_safe(&self, spec: &CommandSpec) -> Result<CommandOutput> {
            self.answer(spec)
        }
        fn spawn_process(&self, spec: &CommandSpec) -> Result<host::HostProcess> {
            self.inner.spawn_process(spec)
        }
        fn terminal_launch(&self, cwd: &Path) -> Result<Option<host::TerminalLaunch>> {
            self.inner.terminal_launch(cwd)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ja(version: &VersionInfo) -> Option<String> {
        version_text_with(version, &|key| {
            i18n::translate_in("ja", key).unwrap_or_else(|| panic!("ja に {key} が無い"))
        })
    }

    fn en(version: &VersionInfo) -> Option<String> {
        version_text_with(version, &|key| {
            i18n::translate_in("en", key).unwrap_or_else(|| panic!("en に {key} が無い"))
        })
    }

    fn managed(
        kind: DistributionKind,
        current: Option<&str>,
        target: &str,
        fetchable: bool,
        verified: bool,
    ) -> VersionInfo {
        VersionInfo::Managed {
            kind,
            current: current.map(str::to_string),
            target: target.to_string(),
            fetchable,
            verified,
        }
    }

    /// 版の行の文を固定する（今の版とレジストリの版・次の起動で合わせるか・PATH の物は合わせない・
    /// 道具が無ければ約束しない・binary は検証の値の有無を必ず言う）。
    #[test]
    fn the_version_line_says_the_current_and_the_registry_version() {
        use DistributionKind::{Binary, Npx, Uvx};
        assert_eq!(
            ja(&managed(Npx, Some("0.84.0"), "0.84.0", true, false)).as_deref(),
            Some("0.84.0（レジストリと同じ）")
        );
        assert_eq!(
            ja(&managed(Npx, Some("1.13.1"), "2.1.0", true, false)).as_deref(),
            Some("1.13.1 → 2.1.0（次の起動でレジストリの版にします）")
        );
        assert_eq!(
            ja(&managed(Npx, None, "0.0.34", true, false)).as_deref(),
            Some("レジストリ 0.0.34（最初の起動で取得します）")
        );
        assert_eq!(
            ja(&managed(Npx, Some("1.13.1"), "2.1.0", false, false)).as_deref(),
            Some("1.13.1 · レジストリ 2.1.0"),
            "npx が無ければ「次の起動で」と言わない"
        );
        assert_eq!(
            ja(&managed(Uvx, None, "0.10.1", false, false)).as_deref(),
            Some("レジストリ 0.10.1")
        );
        assert_eq!(
            ja(&managed(Binary, Some("0.9.0"), "0.9.0", true, true)).as_deref(),
            Some("0.9.0 · sha256 で照合済み")
        );
        assert_eq!(
            ja(&managed(Binary, Some("0.9.0"), "0.9.0", true, false)).as_deref(),
            Some("0.9.0 · 検証の値がありません")
        );
        assert_eq!(
            ja(&managed(Binary, Some("0.8.0"), "0.9.0", true, true)).as_deref(),
            Some("0.8.0 → 0.9.0（次の起動で落として sha256 で照合します）")
        );
        assert_eq!(
            ja(&managed(Binary, None, "0.9.0", true, false)).as_deref(),
            Some("レジストリ 0.9.0（最初の起動で落とします・検証の値がありません）")
        );
        assert_eq!(
            ja(&VersionInfo::Path {
                command: "claude-agent-acp".to_string(),
                current: Some("0.70.2".to_string()),
                registry: Some("0.84.0".to_string()),
            })
            .as_deref(),
            Some("0.70.2 · レジストリ 0.84.0（PATH の claude-agent-acp を使います）")
        );
        assert_eq!(
            ja(&VersionInfo::Path {
                command: "opencode".to_string(),
                current: None,
                registry: Some("1.18.34".to_string()),
            })
            .as_deref(),
            Some("PATH の opencode · レジストリ 1.18.34")
        );
        assert_eq!(
            ja(&VersionInfo::Registry {
                version: "1.52.0".to_string()
            })
            .as_deref(),
            Some("レジストリ 1.52.0")
        );
        assert_eq!(
            ja(&VersionInfo::OwnCommand {
                command: "dsh-acp --acp".to_string()
            })
            .as_deref(),
            Some("自分のコマンドで起動（dsh-acp --acp）")
        );
        assert_eq!(ja(&VersionInfo::None), None);
        assert_eq!(
            en(&managed(Npx, Some("1.13.1"), "2.1.0", true, false)).as_deref(),
            Some("1.13.1 → 2.1.0 (moves to the registry version on the next start)")
        );
    }

    /// 足りない物の並びと区切り（ja）。
    #[test]
    fn missing_items_read_in_order() {
        for key in [
            "settings.agent_missing_node",
            "settings.agent_missing_uv",
            "settings.agent_missing_cli",
            "settings.agent_missing_command",
            "settings.agent_missing_login",
            "settings.agent_missing_separator",
            "settings.agent_status_missing",
        ] {
            assert!(i18n::translate_in("ja", key).is_some(), "{key}");
            assert!(i18n::translate_in("en", key).is_some(), "{key}");
        }
        assert_eq!(
            i18n::translate_in("ja", "settings.agent_missing_cli")
                .map(|text| text.replace("%{name}", "copilot")),
            Some("CLI 本体（copilot）".to_string())
        );
    }
}
