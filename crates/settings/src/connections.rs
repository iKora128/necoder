//! 設定 › **接続**（issue #38 H3）: エージェントがモデルの API を呼ぶ口と、その契約のキー。
//!
//! - 接続 = ひな形（GLM Coding Plan・Kimi Code・DeepSeek API …）+ 名前 + 形式（Anthropic 互換 /
//!   OpenAI 互換）+ ベース URL。settings.json の `connections.<id>`（**user の層だけ**）に書く
//! - **キーは OS のキーチェーン**（`acp_client::connections::secrets`）。settings.json には書かない。
//!   入れ方は「クリップボードから貼る」だけで、画面にはキーを出さない（末尾 4 文字と長さだけ）
//! - エージェントごとに使う接続を選ぶ（`agent_connections.<agent id>`）。渡し方の分かるエージェント
//!   （Claude Code・OpenCode・DeepSeek Harness）だけを並べ、受け付けない接続は理由を添えて出さない
//! - **リモート（SSH 先で起こすエージェント）には渡さない**（キーを手元の外へ出さない）旨を面に書く
//!
//! 効くのは次に起動するセッションから（動いているスレッドはそのまま・アカウントの切り替えと同じ）。
//! キーチェーンの読み書きは許可のダイアログで止まりうるので、いつも背景のスレッドで行う。

use super::*;
use acp_client::connections::{self as conn, Harness, Preset, Protocol, Refusal};
use gpui::Focusable as _;

/// 接続ごとのキーの有無（[`SettingsView::refresh_connection_keys`] が背景で読む）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyState {
    Present,
    Missing,
    /// この OS のキーチェーンにはまだ対応していない（Linux）。
    Unavailable,
    /// 引けなかった（キーチェーンの理由）。
    Failed(SharedString),
}

/// 接続を足す / 変えるダイアログ。キーを持つので `Debug` は付けない。
pub(crate) struct ConnectionEditor {
    /// 変えている接続の id（`None` = 新しく足す）。
    pub(crate) editing: Option<String>,
    /// ひな形の id（変える時は変えられない）。
    pub(crate) preset: String,
    pub(crate) protocol: Protocol,
    pub(crate) name: Entity<EditorView>,
    pub(crate) base_url: Entity<EditorView>,
    /// 貼ったキー（画面には出さない）。`None` = 貼っていない（変える時は今のキーのまま）。
    pub(crate) pasted_key: Option<String>,
    /// キーチェーンへ書いている（連打を止める）。
    pub(crate) saving: bool,
    pub(crate) error: Option<SharedString>,
}

/// 一覧・知らせに出す接続の名前（`name` → ひな形の名前（汎用の 2 つは i18n）→ id）。
pub fn connection_name(id: &str, setting: &ConnectionSetting) -> String {
    setting
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| conn::preset(&setting.preset).map(preset_label))
        .unwrap_or_else(|| id.to_string())
}

/// 接続を渡せるエージェント（一覧の順・組み込みが先）。渡し方の分からないエージェントは入らない。
pub fn connection_harnesses(
    catalog: &acp_client::AgentCatalog,
) -> Vec<(acp_client::Agent, Harness)> {
    catalog
        .agents()
        .into_iter()
        .filter_map(|agent| Harness::of(&agent).map(|harness| (agent, harness)))
        .collect()
}

/// 渡せない理由の文（設定の面と、起動を止めた時の transcript の文で同じ物）。
pub fn refusal_text(refusal: Refusal) -> String {
    match refusal {
        Refusal::Protocol { needed } => i18n::t!(
            "settings.connection_refusal_protocol",
            "protocol" => protocol_label(needed)
        ),
        Refusal::OnlyDeepSeek => i18n::t!("settings.connection_refusal_only_deepseek"),
        Refusal::NotAnOpenCodeProvider => i18n::t!("settings.connection_refusal_opencode"),
        Refusal::CustomUrlForOpenCode => i18n::t!("settings.connection_refusal_opencode_url"),
        Refusal::MissingKey => i18n::t!("settings.connection_refusal_missing_key"),
    }
}

/// 形式の名前（Anthropic 互換 / OpenAI 互換）。
fn protocol_label(protocol: Protocol) -> String {
    match protocol {
        Protocol::Anthropic => i18n::t!("settings.connection_protocol_anthropic"),
        Protocol::OpenAi => i18n::t!("settings.connection_protocol_openai"),
    }
}

/// ひな形の名前（製品名・汎用の 2 つは i18n）。
fn preset_label(preset: &Preset) -> String {
    match preset.name {
        Some(name) => name.to_string(),
        None if preset.id == conn::OPENAI_COMPATIBLE => {
            i18n::t!("settings.connection_preset_openai_compatible")
        }
        None => i18n::t!("settings.connection_preset_anthropic_compatible"),
    }
}

/// 画面に出すキーの形（末尾 4 文字と長さだけ）。
fn masked_key(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    i18n::t!(
        "settings.connection_key_pasted",
        "tail" => tail,
        "length" => key.chars().count()
    )
}

/// 新しい接続の id（ひな形の id、重なれば `-2`, `-3` …）。
fn next_connection_id(preset: &str, settings: &Settings) -> String {
    std::iter::once(preset.to_string())
        .chain((2..).map(|number| format!("{preset}-{number}")))
        .find(|id| !settings.connections.contains_key(id))
        .unwrap_or_else(|| preset.to_string())
}

impl SettingsView {
    /// 接続ごとのキーの有無を背景で読み直す（キーチェーンは中身を読まずに引く）。
    pub(crate) fn refresh_connection_keys(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = get(cx).connections.keys().cloned().collect();
        self.connection_keys_generation = self.connection_keys_generation.wrapping_add(1);
        let generation = self.connection_keys_generation;
        if ids.is_empty() {
            self.connection_keys.clear();
            return;
        }
        let store = secret_store(cx);
        cx.spawn(async move |view, cx| {
            let states = cx
                .background_executor()
                .spawn(async move {
                    ids.into_iter()
                        .map(|id| {
                            let state = match store.contains(&id) {
                                Ok(true) => KeyState::Present,
                                Ok(false) => KeyState::Missing,
                                Err(error)
                                    if error
                                        .downcast_ref::<conn::secrets::KeychainUnavailable>()
                                        .is_some() =>
                                {
                                    KeyState::Unavailable
                                }
                                Err(error) => {
                                    KeyState::Failed(SharedString::from(format!("{error:#}")))
                                }
                            };
                            (id, state)
                        })
                        .collect::<std::collections::BTreeMap<_, _>>()
                })
                .await;
            let updated = view.update(cx, |view, cx| {
                if view.connection_keys_generation == generation {
                    view.connection_keys = states;
                    cx.notify();
                }
            });
            if let Err(error) = updated {
                eprintln!("接続のキーの有無を画面へ返せない: {error:#}");
            }
        })
        .detach();
    }

    /// 「接続」のページ。
    pub(crate) fn connections_section(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let note = |text: String| {
            div()
                .px(px(12.))
                .text_size(px(10.5))
                .text_color(theme.fg2)
                .child(SharedString::from(text))
        };
        let mut list = div().flex().flex_col().gap(px(6.));
        if settings.connections.is_empty() {
            list = list.child(
                div()
                    .px(px(12.))
                    .py(px(9.))
                    .rounded(px(8.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.connections_empty"))),
            );
        }
        for (index, (id, setting)) in settings.connections.iter().enumerate() {
            list = list.child(self.connection_card(index, id, setting, settings, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.section_heading(
                i18n::t!("settings.connections_heading"),
                Some(i18n::t!("settings.connections_sub")),
            ))
            // リモートには渡さない（H3 の範囲外）。読み落とされないよう一覧より上に置く。
            .child(note(i18n::t!("settings.connections_remote_note")))
            .child(list)
            .child(self.add_connection_row(cx))
            .child(self.agent_connection_rows(settings, cx))
    }

    /// 接続 1 件のカード（名前・ひな形・形式と宛先・キーの有無・使うエージェント・右に操作）。
    fn connection_card(
        &self,
        index: usize,
        id: &str,
        setting: &ConnectionSetting,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let connection = connection_of(id, setting);
        let preset = connection.preset();
        let preset_text = preset
            .map(preset_label)
            .unwrap_or_else(|| setting.preset.clone());
        let key_line = match (self.connection_keys.get(id), connection.needs_key()) {
            (Some(KeyState::Present), _) => {
                (i18n::t!("settings.connection_key_present"), theme.fg1)
            }
            (Some(KeyState::Missing), true) => {
                (i18n::t!("settings.connection_key_missing"), theme.fg1)
            }
            (Some(KeyState::Missing), false) => {
                (i18n::t!("settings.connection_key_not_needed"), theme.fg2)
            }
            (Some(KeyState::Unavailable), _) => {
                (i18n::t!("settings.connection_key_unavailable"), theme.fg1)
            }
            (Some(KeyState::Failed(reason)), _) => (
                i18n::t!("settings.connection_key_failed", "reason" => reason.as_ref()),
                theme.err,
            ),
            (None, _) => (i18n::t!("settings.connection_key_checking"), theme.fg2),
        };
        let users: Vec<String> = settings
            .agent_connections
            .iter()
            .filter(|(_, connection_id)| connection_id.as_str() == id)
            .map(|(agent_id, _)| {
                agent_catalog(cx)
                    .by_id(agent_id)
                    .map(|agent| agent.label().to_string())
                    .unwrap_or_else(|| agent_id.clone())
            })
            .collect();
        let link = |id: (&'static str, usize), label: String| {
            div()
                .id(id)
                .flex_none()
                .px(px(6.))
                .text_size(px(11.))
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.fg0))
                .child(SharedString::from(label))
        };
        let edit_id = id.to_string();
        let remove_id = id.to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(8.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme.fg0)
                                    .child(SharedString::from(connection.name.clone())),
                            )
                            .when(preset_text != connection.name, |line| {
                                line.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(theme.fg2)
                                        .child(SharedString::from(preset_text.clone())),
                                )
                            }),
                    )
                    .child(
                        link(
                            ("connection-edit", index),
                            i18n::t!("settings.connection_edit"),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, window, cx| {
                                view.open_connection_editor(Some(edit_id.clone()), window, cx)
                            }),
                        ),
                    )
                    .child(
                        link(
                            ("connection-remove", index),
                            i18n::t!("settings.connection_remove"),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, _window, cx| {
                                view.remove_connection(&remove_id, cx)
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(format!(
                        "{} · {}",
                        protocol_label(connection.protocol),
                        connection.base_url
                    ))),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(key_line.1)
                    .child(SharedString::from(key_line.0)),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(if users.is_empty() {
                        i18n::t!("settings.connection_used_by_none")
                    } else {
                        i18n::t!("settings.connection_used_by", "agents" => users.join(", "))
                    })),
            )
    }

    /// 「＋ 接続を追加」の行（エージェントを追加と同じ書式）。
    fn add_connection_row(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        div().flex().items_center().gap(px(10.)).child(
            div()
                .id("connection-add")
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
                .child(SharedString::from(i18n::t!("settings.connections_add")))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, _, window, cx| {
                        view.open_connection_editor(None, window, cx)
                    }),
                ),
        )
    }

    /// エージェントごとに使う接続（渡し方の分かるエージェントだけ）。選択のチップは bg3 + fg0
    /// （識別色は使わない・UI-SPEC §1.3）。受け付けない接続は理由を添えてチップにしない。
    fn agent_connection_rows(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let catalog = agent_catalog(cx);
        let mut rows =
            div().flex().flex_col().gap(px(6.)).child(
                div()
                    .pt(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg1)
                            .child(SharedString::from(i18n::t!(
                                "settings.connections_agents_heading"
                            ))),
                    )
                    .child(div().text_size(px(10.5)).text_color(theme.fg2).child(
                        SharedString::from(i18n::t!("settings.connections_agents_sub")),
                    )),
            );
        let chip = |id: (&'static str, usize), label: String, chosen: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(9.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(11.5))
                .cursor_pointer()
                .when(chosen, |chip| chip.bg(theme.bg3).text_color(theme.fg0))
                .when(!chosen, |chip| {
                    chip.border_1()
                        .border_color(theme.border)
                        .text_color(theme.fg1)
                        .hover(|style| style.text_color(theme.fg0))
                })
                .child(SharedString::from(label))
        };
        let mut chip_index = 0usize;
        for (row, (agent, harness)) in connection_harnesses(&catalog).into_iter().enumerate() {
            let agent_id = agent.id().to_string();
            let chosen = settings.agent_connections.get(&agent_id).cloned();
            let mut chips = div().flex().flex_wrap().items_center().gap(px(4.));
            {
                let agent_id = agent_id.clone();
                chips = chips.child(
                    chip(
                        ("agent-connection", chip_index),
                        i18n::t!("settings.connection_own_login"),
                        chosen.is_none(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.choose_agent_connection(&agent_id, None, cx)
                        }),
                    ),
                );
                chip_index += 1;
            }
            // 渡せない接続は理由ごとにまとめる（同じ理由を接続の数だけ繰り返さない）。
            let mut refused: Vec<(String, Vec<String>)> = Vec::new();
            for (id, setting) in &settings.connections {
                let connection = connection_of(id, setting);
                match harness.accepts(&connection) {
                    Ok(()) => {
                        let (agent_id, id) = (agent_id.clone(), id.clone());
                        chips = chips.child(
                            chip(
                                ("agent-connection", chip_index),
                                connection.name.clone(),
                                chosen.as_deref() == Some(id.as_str()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _, _window, cx| {
                                    view.choose_agent_connection(&agent_id, Some(id.clone()), cx)
                                }),
                            ),
                        );
                        chip_index += 1;
                    }
                    Err(refusal) => {
                        let reason = refusal_text(refusal);
                        match refused.iter_mut().find(|(known, _)| *known == reason) {
                            Some((_, names)) => names.push(connection.name),
                            None => refused.push((reason, vec![connection.name])),
                        }
                    }
                }
            }
            let refused: Vec<String> = refused
                .into_iter()
                .map(|(reason, names)| {
                    i18n::t!(
                        "settings.connection_refused_item",
                        "names" => names.join(", "),
                        "reason" => reason
                    )
                })
                .collect();
            // 手で書いた設定が、渡せない・無い接続を指している（起動はそこで止まる）。
            let broken = chosen
                .as_deref()
                .and_then(|id| match settings.connections.get(id) {
                    None => Some(i18n::t!("settings.connection_choice_missing", "id" => id)),
                    Some(setting) => {
                        harness
                            .accepts(&connection_of(id, setting))
                            .err()
                            .map(|refusal| {
                                i18n::t!(
                                    "settings.connection_choice_refused",
                                    "name" => connection_name(id, setting),
                                    "reason" => refusal_text(refusal)
                                )
                            })
                    }
                });
            rows = rows.child(
                div()
                    .id(("agent-connection-row", row))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(9.))
                    .rounded(px(8.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.fg0)
                            .child(SharedString::from(agent.label().to_string())),
                    )
                    .child(chips)
                    .when(!refused.is_empty(), |row| {
                        row.child(div().text_size(px(10.5)).text_color(theme.fg2).child(
                            SharedString::from(i18n::t!(
                                "settings.connection_refused_list",
                                "list" => refused.join(" · ")
                            )),
                        ))
                    })
                    .when_some(broken, |row, broken| {
                        row.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.err)
                                .child(SharedString::from(broken)),
                        )
                    }),
            );
        }
        rows.child(
            div()
                .px(px(12.))
                .text_size(px(10.5))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!(
                    "settings.connections_other_agents"
                ))),
        )
    }

    /// エージェントが使う接続を選ぶ（`None` = 自分のログイン）。次に起動するセッションから効く。
    pub(crate) fn choose_agent_connection(
        &mut self,
        agent_id: &str,
        connection_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let result = set_agent_connection(cx, agent_id, connection_id.as_deref());
        self.report_save(result, cx);
        cx.notify();
    }

    /// 接続を外す（settings.json から消し、キーチェーンのキーも背景で消す）。使っていたエージェントは
    /// 自分のログインに戻る（指す行も一緒に消える）。
    pub(crate) fn remove_connection(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        let settings = get(cx);
        let name = settings
            .connections
            .get(connection_id)
            .map(|setting| connection_name(connection_id, setting))
            .unwrap_or_else(|| connection_id.to_string());
        let result = set_connection(cx, connection_id, None);
        let removed = result.is_ok();
        self.report_save(result, cx);
        if !removed {
            return;
        }
        let store = secret_store(cx);
        let id = connection_id.to_string();
        cx.spawn(async move |view, cx| {
            let deleted = cx
                .background_executor()
                .spawn(async move { store.delete(&id) })
                .await;
            let updated = view.update(cx, |view, cx| {
                let message = match deleted {
                    Ok(()) => i18n::t!("settings.connection_removed", "name" => &name),
                    Err(error) => i18n::t!(
                        "settings.connection_removed_key_left",
                        "name" => &name,
                        "reason" => format!("{error:#}")
                    ),
                };
                cx.emit(SettingsViewEvent::Notice(SharedString::from(message)));
                view.refresh_connection_keys(cx);
            });
            if let Err(error) = updated {
                eprintln!("接続を外した知らせを出せない: {error:#}");
            }
        })
        .detach();
        cx.notify();
    }

    /// 「＋ 接続を追加」（`None`）/「変える…」（`Some(id)`）: ダイアログを開く。
    pub(crate) fn open_connection_editor(
        &mut self,
        editing: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = get(cx);
        let current = editing
            .as_deref()
            .and_then(|id| settings.connections.get(id).map(|setting| (id, setting)));
        let (preset, protocol, name, base_url) = match current {
            Some((id, setting)) => (
                setting.preset.clone(),
                connection_of(id, setting).protocol,
                connection_name(id, setting),
                setting.base_url.clone(),
            ),
            None => {
                let preset = &conn::PRESETS[0];
                let protocol = preset.protocols()[0];
                (
                    preset.id.to_string(),
                    protocol,
                    preset_label(preset),
                    preset.base_url(protocol).unwrap_or_default().to_string(),
                )
            }
        };
        let theme = self.theme.clone();
        let accent = self.accent;
        let field = |text: String, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                let mut view = EditorView::plain(theme.clone(), accent, true, cx);
                view.set_plain_text(&text, cx);
                view
            });
            cx.subscribe(&input, |view, _input, event, cx| {
                if matches!(event, editor_view::ComposerEvent::Submit) {
                    view.save_connection_editor(cx);
                }
            })
            .detach();
            input
        };
        let name = field(name, cx);
        let base_url = field(base_url, cx);
        window.focus(&name.read(cx).focus_handle(cx), cx);
        self.connection_editor = Some(ConnectionEditor {
            editing: current.map(|(id, _)| id.to_string()),
            preset,
            protocol,
            name,
            base_url,
            pasted_key: None,
            saving: false,
            error: None,
        });
        cx.notify();
    }

    /// 開発用（offscreen 検証）: 接続のページで「接続を追加」を開き、ひな形を選んで偽のキーを貼った
    /// 状態にする（キーの伏せ方の見た目を撮る・キーチェーンには何も書かない）。
    #[cfg(debug_assertions)]
    pub fn debug_open_connection_editor(
        &mut self,
        preset: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_page(SettingsPage::Connections, cx);
        self.open_connection_editor(None, window, cx);
        if !preset.is_empty() {
            self.choose_connection_preset(preset, cx);
        }
        self.accept_connection_key(Some("sk-offscreen-0123456789abcd".to_string()), cx);
    }

    pub(crate) fn close_connection_editor(&mut self, cx: &mut Context<Self>) {
        if self.connection_editor.take().is_some() {
            cx.notify();
        }
    }

    /// ひな形を選ぶ（新しく足す時だけ）。名前と宛先は、人が書き換えていなければひな形の物に入れ替える。
    pub(crate) fn choose_connection_preset(&mut self, preset_id: &str, cx: &mut Context<Self>) {
        let Some(preset) = conn::preset(preset_id) else {
            return;
        };
        let Some(editor) = self.connection_editor.as_mut() else {
            return;
        };
        if editor.editing.is_some() {
            return;
        }
        let previous = conn::preset(&editor.preset);
        let name = editor.name.read(cx).plain_text();
        let base_url = editor.base_url.read(cx).plain_text();
        let untouched_name = name.trim().is_empty()
            || previous.is_some_and(|previous| preset_label(previous) == name.trim());
        let untouched_url = base_url.trim().is_empty()
            || previous
                .and_then(|previous| previous.base_url(editor.protocol))
                .is_some_and(|url| url == base_url.trim());
        let protocol = if preset.base_url(editor.protocol).is_some() {
            editor.protocol
        } else {
            preset.protocols()[0]
        };
        editor.preset = preset.id.to_string();
        editor.protocol = protocol;
        editor.error = None;
        let (name_input, url_input) = (editor.name.clone(), editor.base_url.clone());
        if untouched_name {
            name_input.update(cx, |input, cx| {
                input.set_plain_text(&preset_label(preset), cx)
            });
        }
        if untouched_url {
            let url = preset.base_url(protocol).unwrap_or_default();
            url_input.update(cx, |input, cx| input.set_plain_text(url, cx));
        }
        cx.notify();
    }

    /// 形式を選ぶ。宛先がひな形の口のままなら、その形式の口に入れ替える。
    pub(crate) fn choose_connection_protocol(
        &mut self,
        protocol: Protocol,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.connection_editor.as_mut() else {
            return;
        };
        let preset = conn::preset(&editor.preset);
        if preset.is_some_and(|preset| preset.base_url(protocol).is_none()) {
            return;
        }
        let base_url = editor.base_url.read(cx).plain_text();
        let untouched = base_url.trim().is_empty()
            || preset
                .and_then(|preset| preset.base_url(editor.protocol))
                .is_some_and(|url| url == base_url.trim());
        editor.protocol = protocol;
        editor.error = None;
        if untouched {
            let url = preset
                .and_then(|preset| preset.base_url(protocol))
                .unwrap_or_default();
            let input = editor.base_url.clone();
            input.update(cx, |input, cx| input.set_plain_text(url, cx));
        }
        cx.notify();
    }

    /// 「クリップボードから貼る」: キーを受け取る（画面には出さない）。1 行で空白を含まない物だけ。
    pub(crate) fn paste_connection_key(&mut self, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|item| item.text());
        self.accept_connection_key(text, cx);
    }

    /// 貼られた文字列を確かめてダイアログに持たせる（クリップボードを読む所と分けてテストする）。
    pub(crate) fn accept_connection_key(&mut self, text: Option<String>, cx: &mut Context<Self>) {
        let Some(editor) = self.connection_editor.as_mut() else {
            return;
        };
        let key = text.map(|text| text.trim().to_string()).unwrap_or_default();
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            editor.error = Some(SharedString::from(i18n::t!(
                "settings.connection_key_paste_invalid"
            )));
        } else {
            editor.pasted_key = Some(key);
            editor.error = None;
        }
        cx.notify();
    }

    /// 「保存」: 確かめて、貼ったキーをキーチェーンへ（背景で）→ settings.json へ。キーを書けなければ
    /// settings.json も書かない（キーの無い接続を残さない）。新しい接続の settings.json が書けなければ
    /// 書いたキーを消す。
    pub(crate) fn save_connection_editor(&mut self, cx: &mut Context<Self>) {
        let settings = get(cx);
        let Some(editor) = self.connection_editor.as_mut() else {
            return;
        };
        if editor.saving {
            return;
        }
        let preset = conn::preset(&editor.preset);
        let name = editor.name.read(cx).plain_text().trim().to_string();
        let base_url = editor.base_url.read(cx).plain_text().trim().to_string();
        let needs_key = preset.is_none_or(|preset| preset.needs_key);
        let error = if !conn::is_valid_base_url(&base_url) {
            Some(i18n::t!("settings.connection_url_invalid"))
        } else if editor.editing.is_none() && needs_key && editor.pasted_key.is_none() {
            Some(i18n::t!("settings.connection_key_required"))
        } else {
            None
        };
        if let Some(error) = error {
            editor.error = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        let id = editor
            .editing
            .clone()
            .unwrap_or_else(|| next_connection_id(&editor.preset, &settings));
        let fallback_name = preset.map(preset_label).unwrap_or_else(|| id.clone());
        let setting = ConnectionSetting {
            name: (!name.is_empty() && name != fallback_name).then_some(name),
            preset: editor.preset.clone(),
            protocol: match editor.protocol {
                Protocol::Anthropic => ConnectionProtocol::Anthropic,
                Protocol::OpenAi => ConnectionProtocol::OpenAi,
            },
            base_url,
        };
        let creating = editor.editing.is_none();
        let key = editor.pasted_key.clone();
        editor.saving = true;
        editor.error = None;
        cx.notify();
        let store = secret_store(cx);
        cx.spawn(async move |view, cx| {
            let stored = match key {
                Some(key) => {
                    let (store, id) = (store.clone(), id.clone());
                    cx.background_executor()
                        .spawn(async move { store.set(&id, &key) })
                        .await
                }
                None => Ok(()),
            };
            let updated = view.update(cx, |view, cx| {
                let result = stored.and_then(|()| set_connection(cx, &id, Some(&setting)));
                match result {
                    Ok(()) => {
                        view.connection_editor = None;
                        view.refresh_connection_keys(cx);
                    }
                    Err(error) => {
                        if creating {
                            // 書いたキーだけが残らないように消す（settings.json に無い接続のキー）。
                            let (store, id) = (store.clone(), id.clone());
                            cx.background_executor()
                                .spawn(async move {
                                    if let Err(error) = store.delete(&id) {
                                        eprintln!("書きかけの接続のキーを消せない: {error:#}");
                                    }
                                })
                                .detach();
                        }
                        if let Some(editor) = view.connection_editor.as_mut() {
                            editor.saving = false;
                            editor.error = Some(save_failure_message(&error));
                        }
                    }
                }
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("接続の保存を画面へ返せない: {error:#}");
            }
        })
        .detach();
    }

    /// 接続のダイアログ（設定の面の上・中央・幅 560）。
    pub(crate) fn render_connection_editor(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let editor = self.connection_editor.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent;
        let preset = conn::preset(&editor.preset);
        let chip = |id: (&'static str, usize), label: String, chosen: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(9.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(11.5))
                .cursor_pointer()
                .when(chosen, |chip| chip.bg(theme.bg3).text_color(theme.fg0))
                .when(!chosen, |chip| {
                    chip.border_1()
                        .border_color(theme.border)
                        .text_color(theme.fg1)
                        .hover(|style| style.text_color(theme.fg0))
                })
                .child(SharedString::from(label))
        };
        let label = |text: String| {
            div()
                .text_size(px(11.5))
                .text_color(theme.fg1)
                .child(SharedString::from(text))
        };
        let field = |input: Entity<EditorView>| {
            div()
                .h(px(30.))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg0)
                .overflow_hidden()
                .child(input)
        };
        let button = |id: &'static str, text: String, primary: bool| {
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
                .child(SharedString::from(text))
        };
        let mut presets = div().flex().flex_wrap().gap(px(4.));
        if editor.editing.is_none() {
            for (index, candidate) in conn::PRESETS.iter().enumerate() {
                let preset_id = candidate.id;
                presets = presets.child(
                    chip(
                        ("connection-preset", index),
                        preset_label(candidate),
                        candidate.id == editor.preset,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.choose_connection_preset(preset_id, cx)
                        }),
                    ),
                );
            }
        }
        let protocols = preset
            .map(Preset::protocols)
            .unwrap_or_else(|| vec![editor.protocol]);
        let mut protocol_chips = div().flex().gap(px(4.));
        for (index, protocol) in protocols.into_iter().enumerate() {
            protocol_chips = protocol_chips.child(
                chip(
                    ("connection-protocol", index),
                    protocol_label(protocol),
                    protocol == editor.protocol,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| {
                        view.choose_connection_protocol(protocol, cx)
                    }),
                ),
            );
        }
        let needs_key = preset.is_none_or(|preset| preset.needs_key);
        let key_status = match (&editor.pasted_key, editor.editing.is_some(), needs_key) {
            (Some(key), _, _) => masked_key(key),
            (None, true, _) => i18n::t!("settings.connection_key_keep"),
            (None, false, true) => i18n::t!("settings.connection_key_not_pasted"),
            (None, false, false) => i18n::t!("settings.connection_key_optional"),
        };
        let docs = preset
            .map(|preset| preset.docs)
            .filter(|docs| !docs.is_empty());
        let title = if editor.editing.is_some() {
            i18n::t!("settings.connection_edit_title")
        } else {
            i18n::t!("settings.connection_add_title")
        };
        let card = div()
            .w(px(560.))
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
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _window, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    view.close_connection_editor(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(title)),
            )
            .when(editor.editing.is_none(), |card| {
                card.child(label(i18n::t!("settings.connection_preset"))).child(presets)
            })
            .when(editor.editing.is_some(), |card| {
                card.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!(
                            "settings.connection_preset_fixed",
                            "preset" => preset.map(preset_label).unwrap_or_else(|| editor.preset.clone())
                        ))),
                )
            })
            .child(label(i18n::t!("settings.connection_name")))
            .child(field(editor.name.clone()))
            .child(label(i18n::t!("settings.connection_protocol")))
            .child(protocol_chips)
            .child(label(i18n::t!("settings.connection_base_url")))
            .child(field(editor.base_url.clone()))
            .child(label(i18n::t!("settings.connection_key")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        button("connection-paste-key", i18n::t!("settings.connection_paste_key"), false)
                            .flex_none()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| view.paste_connection_key(cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(theme.fg1)
                            .child(SharedString::from(key_status)),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.connection_key_note"))),
            )
            .when_some(docs, |card, docs| {
                card.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!(
                            "settings.connection_docs",
                            "url" => docs
                        ))),
                )
            })
            .when_some(editor.error.clone(), |card, error| {
                card.child(div().text_size(px(11.5)).text_color(theme.err).child(error))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("connection-cancel", i18n::t!("settings.launch_cancel"), false)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _window, cx| view.close_connection_editor(cx)),
                            ),
                    )
                    .child(
                        button(
                            "connection-save",
                            if editor.saving {
                                i18n::t!("settings.connection_saving")
                            } else {
                                i18n::t!("settings.launch_save")
                            },
                            true,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, _, _window, cx| view.save_connection_editor(cx)),
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
                    cx.listener(|view, _, _window, cx| view.close_connection_editor(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_client::connections::secrets::{MemoryStore, SecretStore};
    use std::sync::Arc;

    fn temp_settings(name: &str, seed: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "necoder-settings-{name}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, seed).expect("seed");
        path
    }

    /// issue #38 H3: 接続を足すと、宛先は settings.json に、**キーはキーチェーン（ここではメモリの
    /// 置き場）にだけ**入る。settings.json のどこにもキーの文字列は出ない。エージェントの選択・外すも
    /// 同じ面から書け、外すとキーも消える。
    #[gpui::test]
    fn a_connection_is_saved_without_its_key_in_settings_json(cx: &mut gpui::TestAppContext) {
        let path = temp_settings("connections", r#"{ "onboarded": true }"#);
        let store = Arc::new(MemoryStore::default());
        cx.update(|cx| {
            init(Some(path.clone()), None, cx);
            install_secret_store(store.clone(), cx);
        });
        let (view, cx) = cx.add_window_view(|_window, cx| {
            let mut view = SettingsView::new(Theme::dark(), gpui::red(), cx);
            view.availability_pending = false;
            view.page = SettingsPage::Connections;
            view
        });
        view.update_in(cx, |view, window, cx| {
            view.open_connection_editor(None, window, cx);
            view.choose_connection_preset("zai-coding-plan", cx);
            // キーを貼らずに保存すると断る（キーの要るひな形）。
            view.save_connection_editor(cx);
            assert!(
                view.connection_editor
                    .as_ref()
                    .and_then(|editor| editor.error.clone())
                    .is_some(),
                "キーが無ければ保存しない"
            );
            view.accept_connection_key(Some("  zk-secret-0123456789  ".into()), cx);
            view.save_connection_editor(cx);
        });
        cx.run_until_parked();
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            !text.contains("zk-secret"),
            "キーが settings.json に出ない: {text}"
        );
        view.update(cx, |view, cx| {
            assert!(view.connection_editor.is_none(), "保存できたら閉じる");
            let settings = get(cx);
            let setting = settings
                .connections
                .get("zai-coding-plan")
                .expect("ひな形の id で足す");
            assert_eq!(setting.base_url, "https://api.z.ai/api/anthropic");
            assert_eq!(setting.protocol, ConnectionProtocol::Anthropic);
            assert_eq!(setting.name, None, "ひな形の名前のままなら書かない");
            assert_eq!(
                view.connection_keys.get("zai-coding-plan"),
                Some(&KeyState::Present)
            );
        });
        assert_eq!(
            store.get("zai-coding-plan").expect("読める").as_deref(),
            Some("zk-secret-0123456789"),
            "キーはキーチェーン（の代わり）にだけある"
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // エージェントの選択（Claude Code）と、外す。
        view.update(cx, |view, cx| {
            view.choose_agent_connection("claude", Some("zai-coding-plan".into()), cx);
            assert_eq!(
                agent_connection(&get(cx), "claude").map(|connection| connection.name),
                Some("GLM Coding Plan (Z.ai)".to_string())
            );
            view.remove_connection("zai-coding-plan", cx);
            let settings = get(cx);
            assert!(settings.connections.is_empty());
            assert!(
                settings.agent_connections.is_empty(),
                "外した接続を指す行も消える（自分のログインに戻る）"
            );
        });
        cx.run_until_parked();
        assert_eq!(
            store.get("zai-coding-plan").expect("読める"),
            None,
            "キーも消える"
        );
        std::fs::remove_file(&path).ok();
    }

    /// 宛先の確かめ（http(s) で始まる）と、2 本目の同じひな形は別の id になる。変える時は今のキーのまま
    /// 名前や URL だけを変えられる。
    #[gpui::test]
    fn base_urls_are_checked_and_ids_do_not_collide(cx: &mut gpui::TestAppContext) {
        let path = temp_settings(
            "connections-ids",
            r#"{ "onboarded": true, "connections": { "deepseek": {
                "preset": "deepseek", "protocol": "anthropic",
                "base_url": "https://api.deepseek.com/anthropic" } } }"#,
        );
        let store = Arc::new(MemoryStore::default());
        store.set("deepseek", "sk-old").expect("seed");
        cx.update(|cx| {
            init(Some(path.clone()), None, cx);
            install_secret_store(store.clone(), cx);
        });
        let (view, cx) = cx.add_window_view(|_window, cx| {
            let mut view = SettingsView::new(Theme::dark(), gpui::red(), cx);
            view.availability_pending = false;
            view
        });
        view.update_in(cx, |view, window, cx| {
            view.open_connection_editor(None, window, cx);
            view.choose_connection_preset("deepseek", cx);
            view.choose_connection_protocol(Protocol::OpenAi, cx);
            let editor = view.connection_editor.as_ref().expect("開いている");
            assert_eq!(
                editor.base_url.read(cx).plain_text(),
                "https://api.deepseek.com",
                "形式を替えるとひな形の口に入れ替わる"
            );
            let url = editor.base_url.clone();
            url.update(cx, |url, cx| url.set_plain_text("api.deepseek.com", cx));
            view.accept_connection_key(Some("sk-new".into()), cx);
            view.save_connection_editor(cx);
            assert!(
                view.connection_editor
                    .as_ref()
                    .and_then(|editor| editor.error.clone())
                    .is_some(),
                "http(s) でない URL は保存しない"
            );
            let url = view
                .connection_editor
                .as_ref()
                .expect("開いている")
                .base_url
                .clone();
            url.update(cx, |url, cx| {
                url.set_plain_text("https://api.deepseek.com", cx)
            });
            view.save_connection_editor(cx);
        });
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            let settings = get(cx);
            assert!(
                settings.connections.contains_key("deepseek-2"),
                "重ならない id"
            );
            assert_eq!(
                settings.connections["deepseek-2"].protocol,
                ConnectionProtocol::OpenAi
            );
            // 変える: 名前だけを変える（キーは貼らない＝今のキーのまま）。
            view.open_connection_editor(Some("deepseek".into()), window, cx);
            let name = view
                .connection_editor
                .as_ref()
                .expect("開いている")
                .name
                .clone();
            name.update(cx, |name, cx| name.set_plain_text("DeepSeek（仕事）", cx));
            view.save_connection_editor(cx);
        });
        cx.run_until_parked();
        view.update(cx, |_view, cx| {
            assert_eq!(
                get(cx).connections["deepseek"].name.as_deref(),
                Some("DeepSeek（仕事）")
            );
        });
        assert_eq!(
            store.get("deepseek").expect("読める").as_deref(),
            Some("sk-old")
        );
        assert_eq!(
            store.get("deepseek-2").expect("読める").as_deref(),
            Some("sk-new")
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        std::fs::remove_file(&path).ok();
    }

    /// エージェントごとの行には、渡し方の分かるエージェントだけが並び、受け付けない接続は理由と一緒に
    /// 「渡せない接続」に回る（チップにしない）。
    #[test]
    fn only_agents_with_a_known_injection_take_connections() {
        let catalog = agent_catalog_for(&Settings::default());
        let harnesses: Vec<(String, Harness)> = connection_harnesses(&catalog)
            .into_iter()
            .map(|(agent, harness)| (agent.id().to_string(), harness))
            .collect();
        assert_eq!(
            harnesses,
            vec![
                ("claude".to_string(), Harness::ClaudeCode),
                ("opencode".to_string(), Harness::OpenCode),
            ]
        );
        let mut settings = Settings::default();
        settings.agent_servers.insert(
            "dsh".to_string(),
            settings_core::AgentServerSetting::Custom {
                name: Some("DeepSeek Harness".to_string()),
                command: "/opt/bin/dsh-acp".to_string(),
                args: Vec::new(),
                env: Default::default(),
            },
        );
        let catalog = agent_catalog_for(&settings);
        assert!(connection_harnesses(&catalog)
            .iter()
            .any(|(agent, harness)| agent.id() == "dsh" && *harness == Harness::DeepSeekHarness));
    }
}
