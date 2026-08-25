//! settings — settings_core（純ロジック・GPUI 非依存）を GPUI アプリに載せる**反応的レイヤ**。
//!
//! 設計（ユーザー確定 2026-07-12）: **settings.json を唯一の真実**にし、UI トグル / CLI / MCP / 手編集は
//! すべてこの 1 つの store の「書き手」にする。3つを別々に作るとズレるので 1 本に集約する。
//! - 真実: [`settings_core::SettingsStore`]（default→user→project の3層マージ）
//! - 反応: [`SettingsGlobal`]（gpui `Global`）。ビューは `cx.observe_global::<SettingsGlobal>` で変化に反応
//! - 監視: user/project の `settings.json` の mtime を ~1.2s poll し、**実際に解決値が変わった時だけ**
//!   `update_global`＝無変化では observer を起こさない（**idle 0% を保つ**）。手編集・別プロセス CLI もこれで反映
//! - in-proc（UI トグル・in-proc MCP）は [`set_user_value`] で即時反映 + 永続化（poll を待たない）
//!
//! 監視は poll（mtime 差分）。真の event-driven（FSEvents/notify）へは後で差し替え可能だが、
//! 2 回の stat / 1.2s は事実上 0% で再描画も起こさない（変化時のみ）。

use gpui::{
    div, prelude::*, px, svg, App, BorrowAppContext, Context, Div, EventEmitter, FontWeight,
    Global, Hsla, IntoElement, MouseButton, Render, SharedString, Stateful, Window,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use theme_core::Theme;

pub use settings_core::{
    persist_agent_default, persist_user_value, user_settings_path, AgentDefaults, Density,
    Settings, SettingsStore,
};

/// poll 間隔。手編集・CLI の反映がこの遅延内に起きる（in-proc は即時なので影響しない）。
const POLL_INTERVAL: Duration = Duration::from_millis(1200);

/// アプリ全体で共有する設定の真実（single source of truth）。
/// UI トグル・CLI・MCP・手編集がすべてここを更新し、`observe_global` で全ビューへ波及する。
pub struct SettingsGlobal {
    store: SettingsStore,
    user_path: Option<PathBuf>,
    project_dir: Option<PathBuf>,
}

impl Global for SettingsGlobal {}

impl SettingsGlobal {
    /// 現在の解決済み設定。
    pub fn settings(&self) -> &Settings {
        self.store.settings()
    }

    /// ファイル群から読み直して store を差し替える。
    fn reload(&mut self) {
        self.store = SettingsStore::load(self.user_path.as_deref(), self.project_dir.as_deref());
    }
}

/// 設定を読み込み、グローバルに載せ、ファイル監視（poll）を開始する。main が起動時に 1 回呼ぶ。
/// `project_dir` があれば `.necoder/settings.json` も監視・マージ対象になる。
pub fn init(user_path: Option<PathBuf>, project_dir: Option<PathBuf>, cx: &mut App) {
    let store = SettingsStore::load(user_path.as_deref(), project_dir.as_deref());
    cx.set_global(SettingsGlobal {
        store,
        user_path: user_path.clone(),
        project_dir: project_dir.clone(),
    });
    // GPUI のアニメーション要素と自前 ticker が同じアクセシビリティ設定を見る。
    // global の live reload にも追従するため、設定画面／CLI／手編集のどれでも即座に静止・再開する。
    let apply_reduce_motion = |cx: &mut App| {
        cx.set_reduce_motion(get(cx).reduce_motion);
    };
    apply_reduce_motion(cx);
    cx.observe_global::<SettingsGlobal>(apply_reduce_motion)
        .detach();
    spawn_watcher(user_path, project_dir, cx);
}

/// 現在の解決済み設定をクローンで取る。ビューは `cx.observe_global::<SettingsGlobal>` で変化に反応する。
/// グローバル未設定（init 前）でも安全に既定を返す。
pub fn get(cx: &App) -> Settings {
    cx.try_global::<SettingsGlobal>()
        .map(|global| global.settings().clone())
        .unwrap_or_default()
}

/// user 設定の 1 キーを更新して**即適用 + 永続化**する（UI トグル・in-proc MCP から）。
/// 書き込み→再読込で observer が発火し、全ビューへ波及する。poll は待たない。
pub fn set_user_value(cx: &mut App, key: &str, value: serde_json::Value) {
    let path = cx
        .try_global::<SettingsGlobal>()
        .and_then(|global| global.user_path.clone());
    if let Some(path) = path {
        if let Err(error) = persist_user_value(&path, key, value) {
            eprintln!("設定の保存に失敗（実行時のみ反映）: {error:#}");
        }
    }
    if cx.has_global::<SettingsGlobal>() {
        cx.update_global::<SettingsGlobal, _>(|global, _| global.reload());
    }
}

/// `agent_defaults.<agent>.<field>` の 1 点を更新して**即適用 + 永続化**する（composer ピルの sticky）。
/// `set_user_value` と同じ経路（書き込み→reload→observer 発火）。agent ごとに保つので Model/Effort/Mode の
/// 値が別 agent へ漏れない。`field` は `"model"` / `"effort"` / `"mode"`。
pub fn set_agent_default(cx: &mut App, agent: &str, field: &str, value: &str) {
    let path = cx
        .try_global::<SettingsGlobal>()
        .and_then(|global| global.user_path.clone());
    if let Some(path) = path {
        if let Err(error) = persist_agent_default(&path, agent, field, value) {
            eprintln!("エージェント既定の保存に失敗（実行時のみ反映）: {error:#}");
        }
    }
    if cx.has_global::<SettingsGlobal>() {
        cx.update_global::<SettingsGlobal, _>(|global, _| global.reload());
    }
}

/// user/project の settings.json を poll 監視し、**解決値が実際に変わった時だけ** global を更新する。
fn spawn_watcher(user_path: Option<PathBuf>, project_dir: Option<PathBuf>, cx: &mut App) {
    let project_file = project_settings_path(project_dir.as_deref());
    cx.spawn(async move |cx| {
        let mut seen_user = mtime(user_path.as_deref());
        let mut seen_project = mtime(project_file.as_deref());
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let now_user = mtime(user_path.as_deref());
            let now_project = mtime(project_file.as_deref());
            if now_user == seen_user && now_project == seen_project {
                continue; // 無変化 → observer を起こさない（idle 0% を保つ）
            }
            seen_user = now_user;
            seen_project = now_project;
            // mtime は変わった。ただし**解決値が本当に変わった時だけ** update_global で observer を発火。
            // （touch や無関係な再保存で無駄に再描画しないため。）app 終了後はこの task 自体が
            // 実行されない（timer await で止まったまま drop される）ので liveness チェックは不要。
            cx.update(|cx| {
                if !cx.has_global::<SettingsGlobal>() {
                    return;
                }
                let global = cx.global::<SettingsGlobal>();
                let fresh =
                    SettingsStore::load(global.user_path.as_deref(), global.project_dir.as_deref());
                let differs = fresh.settings() != global.settings();
                if differs {
                    cx.update_global::<SettingsGlobal, _>(|global, _| global.store = fresh);
                }
            });
        }
    })
    .detach();
}

/// プロジェクト設定ファイルのパス（`<project>/.necoder/settings.json`）。
fn project_settings_path(project_dir: Option<&Path>) -> Option<PathBuf> {
    Some(project_dir?.join(".necoder").join("settings.json"))
}

/// ファイルの最終更新時刻（無ければ `None`）。
fn mtime(path: Option<&Path>) -> Option<SystemTime> {
    std::fs::metadata(path?).ok()?.modified().ok()
}

/// SettingsView から workspace shell へ依頼する操作。
pub enum SettingsViewEvent {
    RunCommand(String),
    OnboardingCompleted,
    /// テーマ選択（外観セクション）。適用は全ビューへの波及が要るので shell（apply_theme）が担う。
    SelectTheme(theme_core::ThemeSource),
    /// user settings.json をエディタタブで開く（Window が要るので shell へ上げる）。
    OpenSettingsJson,
}

/// テーマ保存ディレクトリ（user settings.json と同じ設定フォルダの `themes/`）。
fn themes_dir() -> Option<PathBuf> {
    Some(user_settings_path()?.parent()?.join("themes"))
}

/// 設定ホーム。設定値の保存は自身で行い、Window/Terminal が必要な操作だけ shell へ上げる。
pub struct SettingsView {
    theme: Theme,
    accent: Hsla,
    cli_installed: Vec<bool>,
    auth_states: Vec<acp_client::AgentAuthState>,
    checking_agents: bool,
    availability_generation: u64,
    /// 外観セクションに並べるテーマ一覧（組み込み + 同梱 + ユーザー JSON）。
    /// 描画毎の fs 走査を避けてキャッシュし、設定を開き直すたび [`Self::refresh_availability`] で更新。
    themes: Vec<(SharedString, theme_core::ThemeSource)>,
}

impl SettingsView {
    pub fn new(theme: Theme, accent: Hsla, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            theme,
            accent,
            cli_installed: vec![false; acp_client::AGENTS.len()],
            auth_states: vec![acp_client::AgentAuthState::SignedOut; acp_client::AGENTS.len()],
            checking_agents: true,
            availability_generation: 0,
            themes: theme_core::available_themes(themes_dir().as_deref()),
        };
        view.refresh_availability(cx);
        view
    }

    pub fn set_visuals(&mut self, theme: Theme, accent: Hsla) {
        self.theme = theme;
        self.accent = accent;
    }

    /// vendor CLI の導入・認証確認は Render から分離し、最新世代だけを反映する。
    /// テーマ一覧もここで読み直す（設定を開くたび＝`themes/` に JSON を足した直後も反映される）。
    pub fn refresh_availability(&mut self, cx: &mut Context<Self>) {
        self.themes = theme_core::available_themes(themes_dir().as_deref());
        self.availability_generation = self.availability_generation.wrapping_add(1);
        self.checking_agents = true;
        let generation = self.availability_generation;
        cx.spawn(async move |view, cx| {
            let agent_states = cx
                .background_executor()
                .spawn(async move {
                    let cwd =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    let auth_states = acp_client::refresh_agent_auth_states(cwd).await;
                    let installed = acp_client::AGENTS
                        .iter()
                        .map(acp_client::AgentKind::cli_installed)
                        .collect::<Vec<_>>();
                    (installed, auth_states)
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                if view.availability_generation == generation {
                    view.cli_installed = agent_states.0;
                    view.auth_states = agent_states.1;
                    view.checking_agents = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn set_default_agent(&mut self, label: &str, cx: &mut Context<Self>) {
        set_user_value(
            cx,
            "default_agent",
            serde_json::Value::String(label.to_string()),
        );
        cx.notify();
    }

    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        set_user_value(cx, "onboarded", serde_json::Value::Bool(true));
        cx.emit(SettingsViewEvent::OnboardingCompleted);
        cx.notify();
    }

    fn agent_action_button(
        &self,
        id: (&'static str, usize),
        text: String,
        command: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
            .child(SharedString::from(text))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, _window, cx| {
                    cx.emit(SettingsViewEvent::RunCommand(command.to_string()))
                }),
            )
    }

    // ── Preferences（設定の実効化・M13）──────────────────────────────────────────
    // settings.json を唯一の真実に、UI から各値を直接トグル/調整する。set_user_value が
    // 永続化 + observe_global 波及を担うので、変更は全ビューへ即反映される。

    fn set_pref_bool(&mut self, key: &'static str, value: bool, cx: &mut Context<Self>) {
        set_user_value(cx, key, serde_json::Value::Bool(value));
        cx.notify();
    }

    fn set_pref_int(&mut self, key: &'static str, value: i64, cx: &mut Context<Self>) {
        set_user_value(cx, key, serde_json::json!(value));
        cx.notify();
    }

    fn set_pref_float(&mut self, key: &'static str, value: f64, cx: &mut Context<Self>) {
        set_user_value(cx, key, serde_json::json!(value));
        cx.notify();
    }

    fn set_pref_string(&mut self, key: &'static str, value: &'static str, cx: &mut Context<Self>) {
        set_user_value(cx, key, serde_json::Value::String(value.to_string()));
        cx.notify();
    }

    /// on/off スイッチ（つまみが左右にスライド・on=accent）。
    fn switch(&self, id: (&'static str, usize), on: bool) -> Stateful<Div> {
        let track = if on { self.accent } else { self.theme.bg3 };
        div()
            .id(id)
            .w(px(34.))
            .h(px(18.))
            .rounded(px(9.))
            .bg(track)
            .flex()
            .items_center()
            .px(px(2.))
            .cursor_pointer()
            .when(on, |element| element.justify_end())
            .child(div().size(px(14.)).rounded(px(7.)).bg(gpui::white()))
    }

    /// 設定行の器（左=ラベル+副題 / 右=コントロール）。
    fn pref_row(&self, label: String, sub: Option<String>, control: gpui::AnyElement) -> Div {
        let theme = self.theme.clone();
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
                    .gap(px(1.))
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.fg0)
                            .child(SharedString::from(label)),
                    )
                    .when_some(sub, |element, sub| {
                        element.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from(sub)),
                        )
                    }),
            )
            .child(control)
    }

    fn toggle_row(
        &self,
        key: &'static str,
        idx: usize,
        label: String,
        sub: Option<String>,
        value: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let control = self
            .switch((key, idx), value)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _window, cx| view.set_pref_bool(key, !value, cx)),
            )
            .into_any_element();
        self.pref_row(label, sub, control)
    }

    fn stepper_button(
        &self,
        key: &'static str,
        id: (&'static str, usize),
        glyph: &'static str,
        target: f64,
        is_int: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = self.theme.clone();
        div()
            .id(id)
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.border)
            .text_size(px(13.))
            .text_color(theme.fg1)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
            .child(glyph)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _window, cx| {
                    if is_int {
                        view.set_pref_int(key, target as i64, cx)
                    } else {
                        view.set_pref_float(key, target, cx)
                    }
                }),
            )
    }

    /// 数値ステッパー（[−] 値 [+]・min/max でクランプ）。int/float 兼用。
    fn stepper_row(
        &self,
        key: &'static str,
        label: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        is_int: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let dec = (value - step).max(min);
        let inc = (value + step).min(max);
        let display = format!("{}", value as i64);
        let control = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(self.stepper_button(key, (key, 0), "−", dec, is_int, cx))
            .child(
                div()
                    .min_w(px(28.))
                    .text_size(px(12.5))
                    .text_color(self.theme.fg0)
                    .child(SharedString::from(display)),
            )
            .child(self.stepper_button(key, (key, 1), "+", inc, is_int, cx))
            .into_any_element();
        self.pref_row(label, None, control)
    }

    /// セグメント選択（複数値からひとつ・選択中は accent）。
    fn segmented_row(
        &self,
        key: &'static str,
        label: String,
        options: &[(&'static str, String)],
        current: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let accent = self.accent;
        let mut segments = div().flex().items_center().gap(px(4.));
        for (idx, (value, display)) in options.iter().enumerate() {
            let selected = *value == current;
            let value = *value;
            segments = segments.child(
                div()
                    .id((key, idx))
                    .px(px(9.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.5))
                    .when(selected, |element| {
                        element.bg(accent.alpha(0.16)).text_color(accent)
                    })
                    .when(!selected, |element| {
                        element
                            .text_color(theme.fg2)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    })
                    .child(SharedString::from(display.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.set_pref_string(key, value, cx)
                        }),
                    ),
            );
        }
        self.pref_row(label, None, segments.into_any_element())
    }

    /// 「外観」セクション。テーマをチップの列で並べ、クリックで即適用 + settings.json へ保存する。
    /// 適用（全ビューへの波及）は shell の apply_theme が要るので [`SettingsViewEvent::SelectTheme`] で上げる。
    fn appearance_section(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let accent = self.accent;
        let current = settings.theme.as_str();
        let mut chips = div().flex().flex_wrap().gap(px(4.));
        for (index, (display, source)) in self.themes.iter().enumerate() {
            // settings.json には組み込みなら id（necoder-dark 等）、同梱/ユーザーなら表示名が入る。
            // resolve はどちらでも引けるので、選択中判定も両方に一致させる。
            let selected = current == display.as_ref()
                || matches!(source, theme_core::ThemeSource::BuiltIn(id) if current == *id);
            let source = source.clone();
            chips = chips.child(
                div()
                    .id(("theme-chip", index))
                    .px(px(9.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.5))
                    .when(selected, |element| {
                        element.bg(accent.alpha(0.16)).text_color(accent)
                    })
                    .when(!selected, |element| {
                        element
                            .text_color(theme.fg2)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    })
                    .child(display.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _, _window, cx| {
                            cx.emit(SettingsViewEvent::SelectTheme(source.clone()))
                        }),
                    ),
            );
        }
        let open_json = div()
            .id("open-settings-json")
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
            .child(SharedString::from(i18n::t!("settings.open_json_button")))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _window, cx| cx.emit(SettingsViewEvent::OpenSettingsJson)),
            )
            .into_any_element();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg1)
                            .child(SharedString::from(i18n::t!("settings.appearance_heading"))),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("settings.appearance_sub"))),
                    ),
            )
            .child(self.pref_row(
                i18n::t!("settings.theme_label"),
                Some(i18n::t!("settings.theme_sub")),
                chips.into_any_element(),
            ))
            .child(self.pref_row(
                i18n::t!("settings.open_json"),
                Some(i18n::t!("settings.open_json_sub")),
                open_json,
            ))
    }

    /// 「動作とエディタ」セクション（真実は settings.json・ここは操作面）。
    fn preferences_section(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg1)
                            .child(SharedString::from(i18n::t!("settings.prefs_heading"))),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("settings.prefs_sub"))),
                    ),
            )
            .child(self.toggle_row(
                "submit_on_enter",
                0,
                i18n::t!("settings.pref_submit_on_enter"),
                Some(i18n::t!("settings.pref_submit_on_enter_sub")),
                settings.submit_on_enter,
                cx,
            ))
            .child(self.toggle_row(
                "soft_wrap",
                1,
                i18n::t!("settings.pref_soft_wrap"),
                Some(i18n::t!("settings.pref_soft_wrap_sub")),
                settings.soft_wrap,
                cx,
            ))
            .child(self.toggle_row(
                "format_on_save",
                2,
                i18n::t!("settings.pref_format_on_save"),
                Some(i18n::t!("settings.pref_format_on_save_sub")),
                settings.format_on_save,
                cx,
            ))
            .child(self.toggle_row(
                "agent_auto_name",
                3,
                i18n::t!("settings.pref_agent_auto_name"),
                Some(i18n::t!("settings.pref_agent_auto_name_sub")),
                settings.agent_auto_name,
                cx,
            ))
            .child(self.toggle_row(
                "completion_sound",
                4,
                i18n::t!("settings.pref_completion_sound"),
                Some(i18n::t!("settings.pref_completion_sound_sub")),
                settings.completion_sound,
                cx,
            ))
            .child(self.toggle_row(
                "reduce_motion",
                5,
                i18n::t!("settings.pref_reduce_motion"),
                Some(i18n::t!("settings.pref_reduce_motion_sub")),
                settings.reduce_motion,
                cx,
            ))
            .child(self.stepper_row(
                "font_size",
                i18n::t!("settings.pref_font_size"),
                settings.font_size as f64,
                8.0,
                32.0,
                1.0,
                false,
                cx,
            ))
            .child(self.stepper_row(
                "tab_size",
                i18n::t!("settings.pref_tab_size"),
                settings.tab_size as f64,
                1.0,
                16.0,
                1.0,
                true,
                cx,
            ))
            .child(self.segmented_row(
                "agent_tabs_view",
                i18n::t!("settings.pref_tabs_view"),
                &[
                    ("bar", i18n::t!("settings.tabs_view_bar")),
                    ("list", i18n::t!("settings.tabs_view_list")),
                ],
                &settings.agent_tabs_view,
                cx,
            ))
    }
}

impl EventEmitter<SettingsViewEvent> for SettingsView {}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent;
        let settings = get(cx);
        let default_agent = settings.default_agent.clone();
        let onboarding = !settings.onboarded;
        let mut rows = div().flex().flex_col().gap(px(6.));
        for (index, agent) in acp_client::AGENTS.iter().enumerate() {
            let is_default = agent.label == default_agent;
            let cli_installed = self.cli_installed.get(index).copied().unwrap_or(false);
            let auth_state = self
                .auth_states
                .get(index)
                .copied()
                .unwrap_or(acp_client::AgentAuthState::SignedOut);
            let available = auth_state == acp_client::AgentAuthState::Available;
            let (dot_color, status_text) = if self.checking_agents {
                (theme.fg2, i18n::t!("settings.agent_checking"))
            } else {
                match (cli_installed, auth_state) {
                    (_, acp_client::AgentAuthState::Available) => {
                        (theme.ok, i18n::t!("settings.agent_available"))
                    }
                    (true, acp_client::AgentAuthState::Configured) => {
                        (theme.warn, i18n::t!("settings.agent_configured"))
                    }
                    (true, acp_client::AgentAuthState::SignedOut) => {
                        (theme.fg2, i18n::t!("settings.agent_signed_out"))
                    }
                    (false, _) => (theme.fg2, i18n::t!("settings.not_installed")),
                }
            };
            let default_control = if is_default && available {
                div()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .bg(accent.alpha(0.16))
                    .text_size(px(11.))
                    .text_color(accent)
                    .child(SharedString::from(i18n::t!("settings.is_default")))
                    .into_any_element()
            } else if available {
                let label = agent.label;
                div()
                    .id(("set-default", index))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(SharedString::from(i18n::t!("settings.make_default")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| view.set_default_agent(label, cx)),
                    )
                    .into_any_element()
            } else {
                div().into_any_element()
            };
            let (logo, mono, brand) = agent_brand(agent.id);
            let logo = match logo {
                Some(path) => div()
                    .flex_none()
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(svg().path(path).size(px(20.)).text_color(gpui::rgb(brand)))
                    .into_any_element(),
                None => div()
                    .flex_none()
                    .size(px(26.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::rgb(brand))
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(gpui::white())
                    .child(mono)
                    .into_any_element(),
            };
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(9.))
                    .rounded(px(8.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(if is_default && available {
                        accent.alpha(0.5)
                    } else {
                        theme.border
                    })
                    .child(logo)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.fg0)
                                    .child(agent.label),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(dot_color)
                                    .child(SharedString::from(status_text)),
                            ),
                    )
                    .child(div().flex_1())
                    .child(default_control)
                    .child(if self.checking_agents || available {
                        div().into_any_element()
                    } else if cli_installed {
                        self.agent_action_button(
                            ("agent-login", index),
                            if auth_state == acp_client::AgentAuthState::Configured {
                                i18n::t!("settings.open_cli")
                            } else {
                                i18n::t!("settings.login")
                            },
                            agent.login_cmd,
                            cx,
                        )
                        .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .when(
                        !self.checking_agents && !cli_installed && !available,
                        |row| {
                            row.child(self.agent_action_button(
                                ("agent-install", index),
                                i18n::t!("settings.install"),
                                agent.install_cmd,
                                cx,
                            ))
                        },
                    ),
            );
        }

        let body = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .w_full()
            .max_w(px(680.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(if onboarding {
                                i18n::t!("settings.welcome_title")
                            } else {
                                i18n::t!("settings.title")
                            })),
                    )
                    .when(onboarding, |element| {
                        element.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("settings.welcome_sub"))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg1)
                            .child(SharedString::from(i18n::t!("settings.agents_heading"))),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("settings.agents_sub"))),
                    ),
            )
            .child(rows)
            .when(!onboarding, |element| {
                element
                    .child(self.appearance_section(&settings, cx))
                    .child(self.preferences_section(&settings, cx))
            })
            .when(onboarding, |element| {
                element.child(
                    div()
                        .id("onboarding-start")
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(38.))
                        .rounded(px(8.))
                        .bg(accent)
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.bg0)
                        .cursor_pointer()
                        .hover(|style| style.bg(accent.alpha(0.85)))
                        .child(SharedString::from(i18n::t!("settings.get_started")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, _, _window, cx| view.finish_onboarding(cx)),
                        ),
                )
            });

        div()
            .id("settings-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(theme.bg1)
            .child(
                div()
                    .flex()
                    .justify_center()
                    .px(px(28.))
                    .py(px(24.))
                    .child(body),
            )
    }
}

/// ブランド表示はカタログ（`acp_client::AgentKind`）が単一の出所。設定画面もタブも同じ値を引く。
fn agent_brand(id: &str) -> (Option<&'static str>, &'static str, u32) {
    acp_client::AGENTS
        .iter()
        .find(|agent| agent.id == id)
        .map(|agent| agent.brand())
        .unwrap_or((None, "?", 0x88_88_88))
}
