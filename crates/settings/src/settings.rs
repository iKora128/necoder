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
mod remote;

pub use settings_core::{
    persist_agent_config_default, persist_mcp_enabled, persist_user_value, user_settings_path,
    Density, McpServerSetting, Settings, SettingsStore, UnreadableSettings,
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

/// user 設定ファイルへ 1 点書いて、store を読み直す（observer が発火し全ビューへ波及する）。
/// 書けなかった時も読み直す＝画面はファイルの実際の値へ戻る。失敗は呼び手へ返し、
/// 呼び手が [`save_failure_message`] でトーストに出す（settings.json を黙って壊さない・黙って捨てない）。
fn write_user_file(
    cx: &mut App,
    write: impl FnOnce(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let path = cx
        .try_global::<SettingsGlobal>()
        .and_then(|global| global.user_path.clone());
    let result = match path {
        Some(path) => write(&path),
        None => Ok(()),
    };
    if cx.has_global::<SettingsGlobal>() {
        cx.update_global::<SettingsGlobal, _>(|global, _| global.reload());
    }
    if let Err(error) = &result {
        eprintln!("設定の保存に失敗: {error:#}");
    }
    result
}

/// user 設定の 1 キーを更新して**即適用 + 永続化**する（UI トグル・in-proc MCP から）。
/// 書き込み→再読込で observer が発火し、全ビューへ波及する。poll は待たない。
/// 既存の settings.json を読めない時は書かずに `Err`（[`UnreadableSettings`]）。
pub fn set_user_value(cx: &mut App, key: &str, value: serde_json::Value) -> anyhow::Result<()> {
    write_user_file(cx, |path| persist_user_value(path, key, value))
}

/// `agent_config_defaults.<agent_id>.<config_id>` の 1 点を更新して**即適用 + 永続化**する（composer ピルの sticky）。
/// `set_user_value` と同じ経路（書き込み→reload→observer 発火）。agent ごとに保つので選択が別 agent へ漏れない。
///
/// `config_id` / `value_id` は **ACP が広告した綴りそのまま**を渡すこと（表示名を渡してはいけない）。
/// necoder 側で綴りを作り直すと、次に広告と突き合わせたとき一致せずエージェント既定へ落ちる。
pub fn set_agent_config_default(
    cx: &mut App,
    agent_id: &str,
    config_id: &str,
    value_id: &str,
) -> anyhow::Result<()> {
    write_user_file(cx, |path| {
        persist_agent_config_default(path, agent_id, config_id, value_id)
    })
}

/// `agent_servers.<agent_id>.env.<var>` を更新して**即適用 + 永続化**する（`None` = 消す・O14）。
pub fn set_agent_server_env(
    cx: &mut App,
    agent_id: &str,
    var: &str,
    value: Option<&str>,
) -> anyhow::Result<()> {
    write_user_file(cx, |path| {
        settings_core::persist_agent_server_env(path, agent_id, var, value)
    })
}

/// アカウントのフォルダを指してログインを流すコマンド（POSIX シェル向け・パスは単引用符で囲む）。
pub(crate) fn account_login_command(var: &str, directory: &Path, login: &str) -> String {
    let quoted = directory.to_string_lossy().replace('\'', "'\\''");
    format!("{var}='{quoted}' {login}")
}

/// `<section>.<key>`（`chat.directory` など 1 段の入れ子）を更新して**即適用 + 永続化**する。
/// `set_user_value` と同じ経路（書き込み→reload→observer 発火）。
pub fn set_nested_user_value(
    cx: &mut App,
    section: &str,
    key: &str,
    value: serde_json::Value,
) -> anyhow::Result<()> {
    write_user_file(cx, |path| {
        settings_core::persist_nested_value(path, section, key, value)
    })
}

/// `mcp_servers.<name>.enabled` を更新して**即適用 + 永続化**する（設定画面のトグル）。
/// `set_user_value` と同じ経路（書き込み→reload→observer 発火）。次に開くセッションから効く。
pub fn set_mcp_enabled(cx: &mut App, name: &str, enabled: bool) -> anyhow::Result<()> {
    write_user_file(cx, |path| persist_mcp_enabled(path, name, enabled))
}

/// 設定を保存できなかった時のトーストの文。settings.json を読めない（手編集の途中で壊れている）
/// なら「読めないので保存しなかった」＋理由、それ以外（書き込み失敗）は「保存できなかった」＋理由。
pub fn save_failure_message(error: &anyhow::Error) -> SharedString {
    match error.downcast_ref::<UnreadableSettings>() {
        Some(unreadable) => {
            let file = match unreadable.path.parent().and_then(Path::file_name) {
                Some(folder) if folder == ".necoder" => ".necoder/settings.json".to_string(),
                _ => unreadable
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| unreadable.path.display().to_string()),
            };
            SharedString::from(i18n::t!(
                "settings.save_unreadable",
                "file" => file,
                "reason" => &unreadable.reason
            ))
        }
        None => SharedString::from(i18n::t!(
            "settings.save_failed",
            "reason" => format!("{error:#}")
        )),
    }
}

/// このマシンで使える MCP サーバの一覧（**設定 + 他ツールからの発見**を突き合わせた結果）。
///
/// 設定画面はこれを並べてトグルし、スレッドはこれを [`acp_client::SessionPreferences`] へ渡す
/// ＝画面に出ているものと実際にエージェントへ渡るものが同じ 1 本の解決結果になる。
/// 他ツールの設定ファイルを読む（数 KB の JSON/TOML を 3 本）ので、毎フレームではなく
/// 「設定を開いた時」「セッションを起こす時」に呼ぶこと。
pub fn mcp_servers(cx: &App) -> Vec<acp_client::mcp::McpServerConfig> {
    let settings = get(cx);
    let overrides = settings
        .mcp_servers
        .iter()
        .map(|(name, setting)| (name.clone(), mcp_override(setting)))
        .collect();
    acp_client::mcp::resolve(&overrides, acp_client::mcp::discover())
}

/// 設定スキーマ（`settings_core`）を acp_client の言葉へ写す。
/// 伝送方式の解釈は他ツールの設定を読む時と**同じ 1 つの規則**に委ねる
/// （[`acp_client::mcp::transport_from_parts`]）— 同じ書き方が場所によって別の意味にならない。
fn mcp_override(setting: &McpServerSetting) -> acp_client::mcp::McpServerOverride {
    acp_client::mcp::McpServerOverride {
        enabled: setting.enabled,
        transport: acp_client::mcp::transport_from_parts(
            setting.transport.as_deref(),
            setting.command.as_deref(),
            &setting.args,
            &setting.env,
            setting.url.as_deref(),
            &setting.headers,
        ),
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
    /// 通知音の試聴（選んだ場でその音を鳴らす）。再生は agent_panel::sound なので shell が担う。
    /// `key` は `"sound_done"` / `"sound_waiting"` ＝場面、`value` は選ばれた設定値。
    PreviewSound {
        key: &'static str,
        value: String,
    },
    /// 設定を保存できなかった（settings.json を読めない等）。文は [`save_failure_message`]。
    /// トーストは shell が出す。
    SaveFailed(SharedString),
}

/// 設定ホームのページ（＝左ナビの 1 行。定義順がそのまま並び順・UI-SPEC §12）。
///
/// 選ばれているページは**ビューのメモリ**で、`settings.json` には書かない —
/// 「どのページを見ていたか」は設定値ではない（閉じて開き直せば [`SettingsPage::Agents`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    Agents,
    Mcp,
    Appearance,
    Remote,
    Preferences,
}

impl SettingsPage {
    /// ナビに並べる全ページ（上から順）。
    const ALL: [SettingsPage; 5] = [
        SettingsPage::Agents,
        SettingsPage::Mcp,
        SettingsPage::Appearance,
        SettingsPage::Remote,
        SettingsPage::Preferences,
    ];

    /// ナビのラベル。**ページ本文の見出しと同じキー**を引く（目次と本文で呼び名を割らない）。
    fn heading_key(self) -> &'static str {
        match self {
            SettingsPage::Agents => "settings.agents_heading",
            SettingsPage::Mcp => "settings.mcp_heading",
            SettingsPage::Appearance => "settings.appearance_heading",
            SettingsPage::Remote => "settings.remote_heading",
            SettingsPage::Preferences => "settings.prefs_heading",
        }
    }
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
    /// 導入/ログインボタン押下後の変化見張り中フラグ（agent ごと・多重起動防止）。
    watching_agents: Vec<bool>,
    /// vendor CLI の調査を次の描画で 1 回蹴る（[`Self::refresh_availability`] が立て、render が倒す）。
    /// **調査は実 CLI を起動する**（`probe_acp_session`）ので、開かれてもいない設定ホームのために
    /// 子プロセスを撒かない。設定ホームが実際に描かれた時だけ走らせるための 1 bit。
    availability_pending: bool,
    /// 外観セクションに並べるテーマ一覧（組み込み + 同梱 + ユーザー JSON）。
    /// 描画毎の fs 走査を避けてキャッシュし、設定を開き直すたび [`Self::refresh_availability`] で更新。
    themes: Vec<(SharedString, theme_core::ThemeSource)>,
    /// 表示中のページ（左ナビの選択）。永続化しない（[`SettingsPage`]）。
    page: SettingsPage,
    /// MCP サーバの一覧（設定 + 他ツールからの発見）。描画毎に 3 本のファイルを読まないよう
    /// キャッシュし、設定を開き直すたび / トグルするたびに [`Self::refresh_availability`] で更新。
    mcp_servers: Vec<acp_client::mcp::McpServerConfig>,
    /// MCP ページの絞り込み: 出所（`None` = すべて）。
    mcp_filter_source: Option<acp_client::mcp::McpSource>,
    /// MCP ページの絞り込み: 有効なものだけ表示する。
    mcp_filter_enabled_only: bool,
    /// エージェントごとのアカウントのフォルダ名（`acp_client::AGENTS` と同じ並び・O14）。
    /// 描画ごとに fs を読まないよう、開き直すたび / 作るたびに [`Self::refresh_lists`] で読む。
    accounts: Vec<Vec<String>>,
    /// ターミナル用 `ne` シム（cli_shim crate）の設置状態。`Some(実体パス)` = 設置済み。
    cli_shim_target: Option<PathBuf>,
    /// `ne` シムの設置/削除を実行中（連打防止・「実行中…」表示）。
    cli_shim_busy: bool,
    /// `ne` シムの直近の失敗（管理者ダイアログのキャンセル等）。行の下に赤字で出す。
    cli_shim_error: Option<SharedString>,
    remote_pairing: Option<remote::Pairing>,
    remote_busy: bool,
    remote_error: Option<SharedString>,
    remote_generation: u64,
    /// Skills 節の中身（skill 置き場の走査結果と necoder の skill の状態）。`None` = まだ読んでいない。
    /// 他人の置き場を数十個読むので背景で読み、描画はこのキャッシュだけを見る。
    skills: Option<SkillsSnapshot>,
    /// 一覧に含めるプロジェクト（アクティブなローカルのプロジェクトの根）。workspace が開く前に渡す。
    skills_project: Option<PathBuf>,
    /// necoder の skill を置いている最中（連打防止・「実行中…」表示）。
    skills_busy: bool,
    /// 直近の設置の失敗。行の下に出す。
    skills_error: Option<SharedString>,
    skills_generation: u64,
    #[cfg(feature = "remote-preview")]
    remote_preview_only: bool,
}

/// Skills 節に出すもの（背景で 1 度に読む）。
#[derive(Clone)]
struct SkillsSnapshot {
    /// 表示で `~/…` に縮めるためのホーム。
    home: PathBuf,
    skills: Vec<agent_skills::SkillEntry>,
    /// front matter が壊れていて一覧から外した数。
    skipped: usize,
    /// necoder の skill の置き場（`--agent` 省略時と同じ入れ先）ごとの状態。
    placements: Vec<agent_skills::StubPlacement>,
}

/// necoder の skill の行に出す操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NecoderSkillAction {
    /// necoder が書いた古い版がある（「更新があります」+ 更新する）。無い置き場があれば一緒に置く。
    Update,
    /// 置いていない置き場がある（入れる）。
    Install,
    /// どの置き場も最新（インストール済み）。
    Installed,
    /// 出す操作が無い（人が書いた SKILL.md だけ・読めない等。注記だけを出す）。
    Nothing,
}

/// 置き場ごとの状態から行の操作を決める（読めなかった置き場は `states` に入れない）。
fn necoder_skill_action(states: &[agent_skills::StubState]) -> NecoderSkillAction {
    use agent_skills::StubState;
    if states.contains(&StubState::Outdated) {
        NecoderSkillAction::Update
    } else if states.contains(&StubState::Missing) {
        NecoderSkillAction::Install
    } else if !states.is_empty() && states.iter().all(|state| *state == StubState::Current) {
        NecoderSkillAction::Installed
    } else {
        NecoderSkillAction::Nothing
    }
}

/// Skills 節の中身を読む（背景スレッドから）。ホームが分からなければ `None`。
fn load_skills(project: Option<&Path>) -> Option<SkillsSnapshot> {
    let home = agent_skills::default_home()?;
    let scan = agent_skills::scan(&agent_skills::skill_roots(&home, project));
    let placements = match cli_shim::current_binary() {
        Ok(binary) => agent_skills::user_stub_placements(
            &home,
            &agent_skills::default_agents(&home),
            &agent_skills::stub_skill_md(&binary),
        ),
        Err(error) => {
            eprintln!("necoder の skill の状態を確かめられない: {error:#}");
            Vec::new()
        }
    };
    Some(SkillsSnapshot {
        home,
        skills: scan.skills,
        skipped: scan.skipped.len(),
        placements,
    })
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
            watching_agents: vec![false; acp_client::AGENTS.len()],
            availability_pending: false,
            themes: theme_core::available_themes(themes_dir().as_deref()),
            page: SettingsPage::Agents,
            mcp_servers: Vec::new(),
            mcp_filter_source: None,
            mcp_filter_enabled_only: false,
            accounts: Vec::new(),
            cli_shim_target: None,
            cli_shim_busy: false,
            cli_shim_error: None,
            remote_pairing: None,
            remote_busy: false,
            remote_error: None,
            remote_generation: 0,
            skills: None,
            skills_project: None,
            skills_busy: false,
            skills_error: None,
            skills_generation: 0,
            #[cfg(feature = "remote-preview")]
            remote_preview_only: false,
        };
        view.refresh_availability(cx);
        view
    }

    pub fn set_visuals(&mut self, theme: Theme, accent: Hsla) {
        self.theme = theme;
        self.accent = accent;
    }

    /// テーマ / MCP の一覧を読み直す（同期・fs のみ）。描画毎の fs 走査を避けるキャッシュの更新。
    fn refresh_lists(&mut self, cx: &mut Context<Self>) {
        self.themes = theme_core::available_themes(themes_dir().as_deref());
        self.mcp_servers = mcp_servers(cx);
        self.accounts = acp_client::AGENTS
            .iter()
            .map(|agent| {
                settings_core::accounts_root(agent.id)
                    .map(|root| settings_core::list_accounts(&root))
                    .unwrap_or_default()
            })
            .collect();
    }

    // ── アカウント切替（O14）─────────────────────────────────────────────────────

    /// このエージェントで使うアカウントを選ぶ（`None` = 既定のアカウント＝環境変数を消す）。
    /// 効くのは次に起動するエージェントから（動いているスレッドは今のアカウントのまま）。
    fn use_account(
        &mut self,
        agent_id: &'static str,
        account: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(var) = settings_core::account_env_var(agent_id) else {
            return;
        };
        let directory = account
            .and_then(|name| settings_core::accounts_root(agent_id).map(|root| root.join(name)));
        let value = directory
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let result = set_agent_server_env(cx, agent_id, var, value.as_deref());
        self.report_save(result, cx);
        cx.notify();
    }

    /// ＋ 新しいアカウント: 置き場に空のフォルダを作って選び、公式 CLI のログインをターミナルで流す。
    fn add_account(&mut self, agent_index: usize, cx: &mut Context<Self>) {
        let Some(agent) = acp_client::AGENTS.get(agent_index) else {
            return;
        };
        let Some(root) = settings_core::accounts_root(agent.id) else {
            return;
        };
        let existing = self.accounts.get(agent_index).cloned().unwrap_or_default();
        let name = settings_core::next_account_name(&existing);
        let directory = root.join(&name);
        if let Err(error) = std::fs::create_dir_all(&directory) {
            cx.emit(SettingsViewEvent::SaveFailed(SharedString::from(format!(
                "{}: {error}",
                directory.display()
            ))));
            return;
        }
        self.refresh_lists(cx);
        self.use_account(agent.id, Some(name), cx);
        self.login_account(agent_index, &directory, cx);
    }

    /// 選んだアカウントのフォルダで公式 CLI のログインを流す（ターミナル・資格情報は CLI が自分で扱う）。
    fn login_account(&mut self, agent_index: usize, directory: &Path, cx: &mut Context<Self>) {
        let Some(agent) = acp_client::AGENTS.get(agent_index) else {
            return;
        };
        let (Some(var), Some(login)) = (
            settings_core::account_env_var(agent.id),
            agent.login_command(),
        ) else {
            return;
        };
        self.watch_agent_progress(agent_index, cx);
        cx.emit(SettingsViewEvent::RunCommand(account_login_command(
            var, directory, login,
        )));
    }

    /// 設定ホームを開く / 開き直す時に呼ぶ。一覧はここで即座に読み直し（`themes/` に JSON を足した
    /// 直後も反映される）、**vendor CLI の調査は次の描画に予約する**（[`Self::availability_pending`]）。
    /// 調査は実 CLI を起動するので「開いた」ではなく「描かれた」を合図にする。
    pub fn refresh_availability(&mut self, cx: &mut Context<Self>) {
        self.refresh_lists(cx);
        self.checking_agents = true;
        self.availability_pending = true;
        cx.notify();
    }

    /// 予約済みの vendor CLI 調査を 1 回だけ走らせる（render から。最新世代だけを反映する）。
    fn probe_availability(&mut self, cx: &mut Context<Self>) {
        self.availability_pending = false;
        // skill の置き場も「描かれた」を合図に読み直す（開かれていない設定のために fs を歩かない）。
        self.refresh_skills(cx);
        self.availability_generation = self.availability_generation.wrapping_add(1);
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
                    (installed, auth_states, cli_shim::installed_target())
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                if view.availability_generation == generation {
                    view.cli_installed = agent_states.0;
                    view.auth_states = agent_states.1;
                    view.cli_shim_target = agent_states.2;
                    view.checking_agents = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 導入/ログイン後の反映ラグ対策。ボタン押下後だけ 1 秒間隔の軽量チェック
    /// （PATH/資格情報ファイルの stat のみ・子プロセス無し）を回し、変化を検知した瞬間に
    /// [`Self::refresh_availability`] へ切り替える。設定を開き直さなくても「利用可能」まで進む。
    fn watch_agent_progress(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(agent) = acp_client::AGENTS.get(index) else {
            return;
        };
        if self.watching_agents.get(index).copied().unwrap_or(true) {
            return; // 既に見張り中（ボタン連打・導入とログインの重複起動を防ぐ）
        }
        self.watching_agents[index] = true;
        let baseline = (agent.cli_installed(), agent.configured_auth_state());
        cx.spawn(async move |view, cx| {
            // npm 導入は数分掛かり得るので 10 分まで見張る（stat 数回/秒＝実質タダ）。
            let mut changed = false;
            for _ in 0..600 {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let current = cx
                    .background_executor()
                    .spawn(async move { (agent.cli_installed(), agent.configured_auth_state()) })
                    .await;
                if current != baseline {
                    changed = true;
                    break;
                }
            }
            let _ = view.update(cx, |view, cx| {
                if let Some(flag) = view.watching_agents.get_mut(index) {
                    *flag = false;
                }
                if changed {
                    view.refresh_availability(cx);
                }
            });
        })
        .detach();
    }

    /// 設定の保存結果を見る。失敗は shell にトーストを頼む（黙って捨てない）。
    fn report_save(&mut self, result: anyhow::Result<()>, cx: &mut Context<Self>) {
        if let Err(error) = result {
            cx.emit(SettingsViewEvent::SaveFailed(save_failure_message(&error)));
        }
    }

    fn set_default_agent(&mut self, label: &str, cx: &mut Context<Self>) {
        let result = set_user_value(
            cx,
            "default_agent",
            serde_json::Value::String(label.to_string()),
        );
        self.report_save(result, cx);
        cx.notify();
    }

    /// Captain の任命 / 解任（FLEET-V2 §5.7）。同じエージェントをもう一度押すと解任（`null`）。
    /// 任命は人の明示操作に限る（既定ドリフト禁止・DECISIONS §8）ので、既定エージェントとは連動させない。
    fn toggle_captain(&mut self, label: &str, current: Option<&str>, cx: &mut Context<Self>) {
        let result = set_user_value(cx, "captain_agent", next_captain_value(label, current));
        self.report_save(result, cx);
        cx.notify();
    }

    /// 「AI エージェント」ページを開いた状態にする（Fleet の Captain 行「任命する」の行き先）。
    pub fn show_agents_page(&mut self, cx: &mut Context<Self>) {
        self.select_page(SettingsPage::Agents, cx);
    }

    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        let result = set_user_value(cx, "onboarded", serde_json::Value::Bool(true));
        self.report_save(result, cx);
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
        let agent_index = id.1;
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
                cx.listener(move |view, _, _window, cx| {
                    // コマンド完了イベントは取れない（terminal はシェルに落ちる）ので、
                    // 押した時点から変化見張りを開始して反映ラグを消す。
                    view.watch_agent_progress(agent_index, cx);
                    cx.emit(SettingsViewEvent::RunCommand(command.to_string()))
                }),
            )
    }

    // ── コマンドライン（`ne` シム・cli_shim crate）───────────────────────────────

    /// `ne` シムの設置/削除を背景で実行する。/usr/local/bin に書けない時は cli_shim が
    /// 管理者ダイアログ（osascript）へ倒す＝UI スレッドは待たない。結果で行の状態を更新。
    fn run_cli_shim_action(&mut self, install: bool, cx: &mut Context<Self>) {
        if self.cli_shim_busy {
            return; // 連打防止（ダイアログの多重表示を防ぐ）
        }
        self.cli_shim_busy = true;
        self.cli_shim_error = None;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let prompt = if install {
                i18n::t!("settings.cli_admin_prompt")
            } else {
                i18n::t!("settings.cli_admin_prompt_remove")
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    if install {
                        let binary = cli_shim::current_binary()?;
                        cli_shim::install_with_admin_prompt(&binary, &prompt)?;
                    } else {
                        cli_shim::uninstall_with_admin_prompt(&prompt)?;
                    }
                    Ok::<_, anyhow::Error>(cli_shim::installed_target())
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.cli_shim_busy = false;
                match result {
                    Ok(target) => view.cli_shim_target = target,
                    Err(error) => {
                        view.cli_shim_error = Some(SharedString::from(format!("{error:#}")))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 「コマンドライン」セクション。`ne <パス>` シムの状態表示と設置/削除
    /// （VSCode の「Shell Command: Install 'code' command in PATH」相当・設置先は /usr/local/bin）。
    fn cli_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let accent = self.accent;
        let shim_path = cli_shim::shim_path().display().to_string();
        let sub = match &self.cli_shim_target {
            Some(target) => i18n::t!(
                "settings.cli_row_sub_installed",
                "path" => shim_path,
                "target" => target.display()
            ),
            None => i18n::t!("settings.cli_row_sub_missing", "path" => shim_path),
        };
        let action_button = |id: &'static str, text: String, install: bool| {
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
                    cx.listener(move |view, _, _window, cx| view.run_cli_shim_action(install, cx)),
                )
        };
        let control = if self.cli_shim_busy || self.checking_agents {
            div()
                .text_size(px(11.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("settings.cli_busy")))
                .into_any_element()
        } else if self.cli_shim_target.is_some() {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .bg(accent.alpha(0.16))
                        .text_size(px(11.))
                        .text_color(accent)
                        .child(SharedString::from(i18n::t!("settings.cli_installed_chip"))),
                )
                .child(action_button(
                    "cli-shim-uninstall",
                    i18n::t!("settings.cli_uninstall_button"),
                    false,
                ))
                .into_any_element()
        } else {
            action_button(
                "cli-shim-install",
                i18n::t!("settings.cli_install_button"),
                true,
            )
            .into_any_element()
        };
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
                            .child(SharedString::from(i18n::t!("settings.cli_heading"))),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("settings.cli_sub"))),
                    ),
            )
            .child(self.pref_row(i18n::t!("settings.cli_row_label"), Some(sub), control))
            .when_some(self.cli_shim_error.clone(), |element, error| {
                element.child(
                    div()
                        .px(px(12.))
                        .text_size(px(10.5))
                        .text_color(self.theme.err)
                        .child(error),
                )
            })
    }

    // ── Skills（エージェントの skill 置き場・agent_skills crate）────────────────────

    /// 一覧に含めるプロジェクト（アクティブなローカルのプロジェクトの根・リモートは `None`）。
    /// workspace が設定を開く直前に渡す。次の読み直しから効く。
    pub fn set_skills_project(&mut self, project: Option<PathBuf>) {
        self.skills_project = project;
    }

    /// Skills 節の中身を背景で読み直す（最新の世代だけを反映する）。
    fn refresh_skills(&mut self, cx: &mut Context<Self>) {
        self.skills_generation = self.skills_generation.wrapping_add(1);
        let generation = self.skills_generation;
        let project = self.skills_project.clone();
        cx.spawn(async move |view, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move { load_skills(project.as_deref()) })
                .await;
            let applied = view.update(cx, |view, cx| {
                if view.skills_generation == generation {
                    view.skills = snapshot;
                    cx.notify();
                }
            });
            if applied.is_err() {
                // 設定画面ごと閉じた＝反映する先が無い。
            }
        })
        .detach();
    }

    /// necoder の skill を、無い置き場へ置き、necoder が書いた古い版を更新する。
    /// 人や別のツールが書いた `SKILL.md`（印が無い）はここからは上書きしない（CLI の `--force` だけ）。
    fn install_necoder_skill(&mut self, cx: &mut Context<Self>) {
        if self.skills_busy {
            return; // 連打防止
        }
        let Some(snapshot) = &self.skills else {
            return;
        };
        let targets: Vec<(PathBuf, bool)> = snapshot
            .placements
            .iter()
            .filter_map(|placement| match placement.state {
                Ok(agent_skills::StubState::Missing) => Some((placement.path.clone(), false)),
                Ok(agent_skills::StubState::Outdated) => Some((placement.path.clone(), true)),
                _ => None,
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        self.skills_busy = true;
        self.skills_error = None;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let desired = agent_skills::stub_skill_md(&cli_shim::current_binary()?);
                    for (path, force) in targets {
                        agent_skills::install_stub(&path, &desired, force)?;
                    }
                    Ok::<_, anyhow::Error>(())
                })
                .await;
            let applied = view.update(cx, |view, cx| {
                view.skills_busy = false;
                if let Err(error) = result {
                    view.skills_error = Some(SharedString::from(format!("{error:#}")));
                }
                view.refresh_skills(cx);
                cx.notify();
            });
            if applied.is_err() {
                // 設定画面ごと閉じた＝結果を見せる先が無い（ファイルは書き終えている）。
            }
        })
        .detach();
    }

    /// 一覧の「どのエージェントが読むか」。製品名はそのまま、共通の置き場とプロジェクトは訳す。
    fn skill_scope_label(scope: agent_skills::SkillScope) -> String {
        match scope {
            agent_skills::SkillScope::ClaudeUser => {
                agent_skills::SkillAgent::ClaudeCode.label().to_string()
            }
            agent_skills::SkillScope::CodexUser => {
                agent_skills::SkillAgent::Codex.label().to_string()
            }
            agent_skills::SkillScope::SharedUser => i18n::t!("settings.skills_scope_shared"),
            agent_skills::SkillScope::ClaudeProject => {
                i18n::t!("settings.skills_scope_claude_project")
            }
            agent_skills::SkillScope::SharedProject => {
                i18n::t!("settings.skills_scope_shared_project")
            }
        }
    }

    /// 「Skills」セクション（AI エージェントのページの末尾）。上 = necoder の skill の設置 / 更新、
    /// 下 = 置き場で見つかった skill の一覧（名前・説明・場所・エージェント）。
    fn skills_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let accent = self.accent;
        let heading = self.section_heading(
            i18n::t!("settings.skills_heading"),
            Some(i18n::t!("settings.skills_sub")),
        );
        let muted = |text: String| {
            div()
                .text_size(px(11.))
                .text_color(theme.fg2)
                .child(SharedString::from(text))
        };
        let Some(snapshot) = &self.skills else {
            return div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(heading)
                .child(self.pref_row(
                    i18n::t!("settings.skills_necoder_label"),
                    None,
                    muted(i18n::t!("settings.skills_checking")).into_any_element(),
                ));
        };
        let states: Vec<agent_skills::StubState> = snapshot
            .placements
            .iter()
            .filter_map(|placement| placement.state.clone().ok())
            .collect();
        let action = necoder_skill_action(&states);
        let action_button = |id: &'static str, text: String| {
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
                    cx.listener(|view, _, _window, cx| view.install_necoder_skill(cx)),
                )
        };
        // 状態に色相を使わない（UI-SPEC §1.3）: 「更新があります」は fg1 の文字、入っている印だけ
        // `ne` コマンドの行と同じ accent-dim のチップ。
        let control =
            if self.skills_busy {
                muted(i18n::t!("settings.skills_busy")).into_any_element()
            } else if action == NecoderSkillAction::Update {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().text_size(px(11.)).text_color(theme.fg1).child(
                        SharedString::from(i18n::t!("settings.skills_update_available")),
                    ))
                    .child(action_button(
                        "skills-update",
                        i18n::t!("settings.skills_update_button"),
                    ))
                    .into_any_element()
            } else if action == NecoderSkillAction::Install {
                action_button("skills-install", i18n::t!("settings.skills_install_button"))
                    .into_any_element()
            } else if action == NecoderSkillAction::Installed {
                div()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .bg(accent.alpha(0.16))
                    .text_size(px(11.))
                    .text_color(accent)
                    .child(SharedString::from(i18n::t!(
                        "settings.skills_installed_chip"
                    )))
                    .into_any_element()
            } else {
                div().into_any_element()
            };
        let placements_sub = snapshot
            .placements
            .iter()
            .map(|placement| {
                format!(
                    "{} {}",
                    placement.agent.label(),
                    agent_skills::display_path(&placement.path, &snapshot.home)
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        let mut notes = div().flex().flex_col().gap(px(2.)).px(px(12.));
        for placement in &snapshot.placements {
            let path = agent_skills::display_path(&placement.path, &snapshot.home);
            let note = match &placement.state {
                Ok(agent_skills::StubState::Foreign) => {
                    Some(i18n::t!("settings.skills_foreign", "path" => path))
                }
                Err(error) => Some(i18n::t!(
                    "settings.skills_read_error",
                    "path" => path,
                    "error" => error
                )),
                Ok(_) => None,
            };
            if let Some(note) = note {
                notes = notes.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(note)),
                );
            }
        }
        let mut list = div().flex().flex_col().gap(px(6.));
        if snapshot.skills.is_empty() {
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
                    .child(SharedString::from(i18n::t!("settings.skills_empty"))),
            );
        }
        for skill in &snapshot.skills {
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(12.))
                    .py(px(8.))
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
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(theme.fg0)
                                    .child(SharedString::from(skill.name.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(Self::skill_scope_label(
                                        skill.scope,
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.fg1)
                            .line_clamp(2)
                            .text_ellipsis()
                            .child(SharedString::from(skill.description.clone())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(agent_skills::display_path(
                                &skill.path,
                                &snapshot.home,
                            ))),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(heading)
            .child(self.pref_row(
                i18n::t!("settings.skills_necoder_label"),
                (!placements_sub.is_empty()).then_some(placements_sub),
                control,
            ))
            .child(notes)
            .when_some(self.skills_error.clone(), |element, error| {
                element.child(
                    div()
                        .px(px(12.))
                        .text_size(px(10.5))
                        .text_color(theme.err)
                        .child(error),
                )
            })
            .child(
                div()
                    .px(px(2.))
                    .pt(px(8.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!(
                        "settings.skills_found",
                        "count" => snapshot.skills.len()
                    ))),
            )
            .child(list)
            .when(snapshot.skipped > 0, |element| {
                element.child(muted(i18n::t!(
                    "settings.skills_skipped",
                    "count" => snapshot.skipped
                )))
            })
    }

    // ── Preferences（設定の実効化・M13）──────────────────────────────────────────
    // settings.json を唯一の真実に、UI から各値を直接トグル/調整する。set_user_value が
    // 永続化 + observe_global 波及を担うので、変更は全ビューへ即反映される。

    fn set_pref_bool(&mut self, key: &'static str, value: bool, cx: &mut Context<Self>) {
        let result = set_user_value(cx, key, serde_json::Value::Bool(value));
        self.report_save(result, cx);
        cx.notify();
    }

    fn set_pref_int(&mut self, key: &'static str, value: i64, cx: &mut Context<Self>) {
        let result = set_user_value(cx, key, serde_json::json!(value));
        self.report_save(result, cx);
        cx.notify();
    }

    fn set_pref_float(&mut self, key: &'static str, value: f64, cx: &mut Context<Self>) {
        let result = set_user_value(cx, key, serde_json::json!(value));
        self.report_save(result, cx);
        cx.notify();
    }

    fn set_pref_string(&mut self, key: &'static str, value: &'static str, cx: &mut Context<Self>) {
        let result = set_user_value(cx, key, serde_json::Value::String(value.to_string()));
        self.report_save(result, cx);
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
    ///
    /// 左列は `flex_1` + **`min_w_0` をセットで**付ける（crate 冒頭の GPUI の罠と同じ）。
    /// `min_w_0` が無いと、長い副題（MCP の stdio コマンドは絶対パスで 100 文字を超える）が
    /// 縮まずに右のコントロールを押し出し、**トグルが幅 0 に潰れて押せなくなる**
    /// （2026-09-10 の offscreen 目視で発見）。コントロール側も潰されないよう縮小を止める。
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
                    .min_w_0()
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
            .child(div().flex_shrink_0().child(control))
    }

    /// 設定行の器（上=ラベル+副題 / 下=コントロールが 1 行まるごと）。
    ///
    /// **横に伸びるコントロール（チップの列）は [`pref_row`] に入れない**。右列は
    /// `flex_shrink_0` なので、チップの合計幅が行を超えると左列だけが幅 0 まで潰れ、
    /// 日本語は空白が無いぶん 1 文字ずつ折り返って**ラベルが縦書きに見える**
    /// （テーマが 7 種に増えて発生・2026-09-13）。上下に分ければ、コントロールは
    /// 行いっぱいを使って `flex_wrap` で素直に折り返せる。
    fn pref_stack(&self, label: String, sub: Option<String>, control: gpui::AnyElement) -> Div {
        let theme = self.theme.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
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
                    .min_w_0()
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
            .child(div().w_full().min_w_0().child(control))
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
        self.segmented_row_with(key, label, None, options, current, false, cx)
    }

    /// セグメント行の本体。`preview` を立てると、押したときに保存に加えて
    /// [`SettingsViewEvent::PreviewSound`] を上げる（通知音は聴かないと選べない）。
    #[allow(clippy::too_many_arguments)]
    fn segmented_row_with(
        &self,
        key: &'static str,
        label: String,
        sub: Option<String>,
        options: &[(&'static str, String)],
        current: &str,
        preview: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme.clone();
        let accent = self.accent;
        // 通知音のように選択肢が増える行があるので折り返す（はみ出して押せなくなるのを防ぐ）。
        let mut segments = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap(px(4.));
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
                            view.set_pref_string(key, value, cx);
                            if preview {
                                cx.emit(SettingsViewEvent::PreviewSound {
                                    key,
                                    value: value.to_string(),
                                });
                            }
                        }),
                    ),
            );
        }
        self.pref_row(label, sub, segments.into_any_element())
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
            // チップは数がテーマの数だけ増える＝右列に収まらない。上下 2 段の器を使う。
            .child(self.pref_stack(
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

    // ── AI エージェント（ページ「AI エージェント」）──────────────────────────

    /// エージェント一覧の行。**ページとオンボーディングで共用する**（初回はナビ無しの
    /// 1 枚スクロールに同じ行が出る・UI-SPEC §12）。
    /// `show_captain` = Captain の任命ボタンを出すか（オンボーディングでは出さない＝初回に Fleet の概念を持ち込まない）。
    fn agents_rows(&self, settings: &Settings, show_captain: bool, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let accent = self.accent;
        let default_agent = settings.default_agent.clone();
        let captain_agent = settings.captain_agent.clone();
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
                    .flex_none()
                    .whitespace_nowrap()
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
                    .flex_none()
                    .whitespace_nowrap()
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
            // 任命済みの行は利用不可（ログアウト等）でも出す＝解任の手段を消さない。
            let is_captain = captain_agent.as_deref() == Some(agent.label);
            let captain_control = if show_captain && (available || is_captain) {
                let label = agent.label;
                let current = captain_agent.clone();
                div()
                    .id(("set-captain", index))
                    .flex_none()
                    .whitespace_nowrap()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .when(is_captain, |element| {
                        element.bg(accent.alpha(0.16)).text_color(accent)
                    })
                    .when(!is_captain, |element| {
                        element
                            .text_color(theme.fg2)
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    })
                    .child(SharedString::from(if is_captain {
                        i18n::t!("settings.is_captain")
                    } else {
                        i18n::t!("settings.make_captain")
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.toggle_captain(label, current.as_deref(), cx)
                        }),
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
                    // 名前の列だけが縮む（右のボタン群をカードの外へ押し出さない）。長い名前は折り返す。
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
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
                    .child(captain_control)
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
            // アカウント切替（O14・Claude Code / Codex）。ログインは POSIX シェルで流すので Windows は未対応。
            if let Some(var) =
                settings_core::account_env_var(agent.id).filter(|_| cli_installed && !cfg!(windows))
            {
                rows = rows.child(self.account_line(index, agent.id, var, settings, cx));
            }
        }
        rows
    }

    /// 1 エージェント分のアカウントの行: 既定 / 作ったアカウント / ＋ 新しいアカウント / ログイン。
    /// 選ぶと `agent_servers.<id>.env.<var>` を書く（資格情報は読まない・置き場を指すだけ）。
    fn account_line(
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
        let chip = |id: (&'static str, usize), label: String, chosen: bool| {
            div()
                .id(id)
                .px(px(8.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(11.))
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
        let mut line = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .pl(px(48.))
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.account_label"))),
            )
            .child(
                chip(
                    ("account-default", index),
                    i18n::t!("settings.account_default"),
                    current.is_none(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| view.use_account(agent_id, None, cx)),
                ),
            );
        for (position, name) in accounts.iter().enumerate() {
            let chosen = selected.as_deref() == Some(name.as_str());
            let account = name.clone();
            line = line.child(
                chip(("account", index * 100 + position), name.clone(), chosen).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| {
                        view.use_account(agent_id, Some(account.clone()), cx)
                    }),
                ),
            );
        }
        // settings.json で necoder の置き場の外を指している（手で書いた）: 選ばれていることだけ見せる。
        if current.is_some() && selected.is_none() {
            line = line.child(chip(
                ("account-elsewhere", index),
                i18n::t!("settings.account_elsewhere"),
                true,
            ));
        }
        line = line.child(
            chip(
                ("account-add", index),
                i18n::t!("settings.account_add"),
                false,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _window, cx| view.add_account(index, cx)),
            ),
        );
        if let Some(path) = current.clone() {
            line = line.child(
                div()
                    .id(("account-login", index))
                    .px(px(6.))
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0))
                    .child(SharedString::from(i18n::t!("settings.account_login")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| {
                            view.login_account(index, Path::new(&path), cx)
                        }),
                    ),
            );
        }
        line.child(
            div()
                .w_full()
                .text_size(px(10.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("settings.account_hint"))),
        )
    }

    // ── ページの器（左ナビ + ページ面）──────────────────────────────────────────

    /// ナビの行を押した。**ページの選択は永続化しない**（[`SettingsPage`]）。
    /// 開発用（offscreen 検証）: ページを指定して開く。`"prefs"` / `"mcp"` / `"agents"` …
    #[cfg(debug_assertions)]
    pub fn debug_select_page(&mut self, page: &str, cx: &mut Context<Self>) {
        let page = match page {
            "prefs" => SettingsPage::Preferences,
            "mcp" => SettingsPage::Mcp,
            "appearance" => SettingsPage::Appearance,
            "remote" => SettingsPage::Remote,
            _ => SettingsPage::Agents,
        };
        self.select_page(page, cx);
    }

    fn select_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        if self.page != page {
            self.page = page;
            cx.notify();
        }
    }

    /// セクション見出し（太字ラベル + 副題）。ナビのラベルと同じキーを引く側の相方。
    fn section_heading(&self, title: String, sub: Option<String>) -> Div {
        let theme = self.theme.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg1)
                    .child(SharedString::from(title)),
            )
            .when_some(sub, |element, sub| {
                element.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(sub)),
                )
            })
    }

    /// 左ナビ（ページの目次）。**スクロールしない** — どこに居るかが常に見える（UI-SPEC §12）。
    fn nav_column(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let accent = self.accent;
        let enabled = self
            .mcp_servers
            .iter()
            .filter(|server| server.enabled)
            .count();
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .flex_none()
            .w(px(184.))
            .h_full()
            .px(px(12.))
            .py(px(24.))
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px(px(8.))
                    .pb(px(10.))
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!("settings.title"))),
            );
        for (index, page) in SettingsPage::ALL.iter().enumerate() {
            let page = *page;
            let selected = self.page == page;
            // MCP だけは「有効/全体」を添える＝開かなくても状態が見える。
            let badge = (page == SettingsPage::Mcp && !self.mcp_servers.is_empty())
                .then(|| format!("{enabled}/{}", self.mcp_servers.len()));
            column = column.child(
                div()
                    .id(("settings-nav", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    // 非選択でも 2px を敷いて（透明）、選択で文字位置がずれないようにする。
                    .border_l_2()
                    .border_color(if selected {
                        accent
                    } else {
                        gpui::transparent_black()
                    })
                    .when(selected, |element| {
                        element.bg(theme.bg3).text_color(theme.fg0)
                    })
                    .when(!selected, |element| {
                        element
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(i18n::t!(page.heading_key()))),
                    )
                    .when_some(badge, |element, badge| {
                        element.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(badge)),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| view.select_page(page, cx)),
                    ),
            );
        }
        column
    }

    /// 選ばれているページの中身。1 ページ = 1 セクション（エージェントだけ `ne` コマンドと Skills を伴う）。
    fn page_body(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        match self.page {
            SettingsPage::Agents => div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(self.section_heading(
                            i18n::t!("settings.agents_heading"),
                            Some(i18n::t!("settings.agents_sub")),
                        ))
                        .child(self.agents_rows(settings, true, cx)),
                )
                // 導入系（エージェント CLI）の直後に `ne` コマンドを並べる。
                // Windows は W フェーズまで非対応＝セクションごと出さない。
                .when(cli_shim::supported(), |element| {
                    element.child(self.cli_section(cx))
                })
                // エージェントが読む手順書（SKILL.md）。`ne` の次＝「エージェントに necoder を教える」の並び。
                .child(self.skills_section(cx)),
            SettingsPage::Mcp => self.mcp_section(cx),
            SettingsPage::Appearance => self.appearance_section(settings, cx),
            SettingsPage::Remote => self.remote_section(cx),
            SettingsPage::Preferences => self.preferences_section(settings, cx),
        }
    }

    // ── MCP サーバ（ACP セッションへ渡す道具）────────────────────────────────────

    /// 1 件の on/off を保存し、一覧を読み直す（次に開くスレッドのセッションから効く）。
    fn toggle_mcp_server(&mut self, name: String, enabled: bool, cx: &mut Context<Self>) {
        let result = set_mcp_enabled(cx, &name, enabled);
        self.report_save(result, cx);
        self.mcp_servers = mcp_servers(cx);
        cx.notify();
    }

    /// 「MCP サーバ」セクション。necoder の設定に書いたものと、他ツール（Codex CLI /
    /// Claude Code / Cursor）の設定から**発見**したものを 1 つの一覧に並べ、トグルで有効化する。
    ///
    /// ACP は「どの MCP サーバへ繋ぐか」をクライアントが決めるプロトコルなので、ここで on に
    /// したものだけがエージェントから見える（エージェント側の設定ファイルはセッションに出てこない）。
    /// 発見しただけのものが既定 off なのは、他人の設定を根拠に子プロセスを起こしたり課金される
    /// リモートサーバへ繋いだりしないため。
    fn mcp_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let total = self.mcp_servers.len();
        let enabled_count = self
            .mcp_servers
            .iter()
            .filter(|server| server.enabled)
            .count();
        // claude.ai アカウントのコネクタは necoder の一覧を通らずに Claude のスレッドへ入ってくる
        // （ここで選んだ物だけを渡す、の唯一の例外）。だからこのページの一番上で切れるようにする。
        let connectors = get(cx).claude_ai_connectors;
        let mut rows = div().flex().flex_col().gap(px(6.));
        let mut shown = 0usize;
        for (index, server) in self.mcp_servers.iter().enumerate() {
            if !self.mcp_row_visible(server) {
                continue;
            }
            shown += 1;
            let name = server.name.clone();
            let enabled = server.enabled;
            let source = mcp_source_label(server.source);
            let control = self
                .switch(("mcp-server", index), enabled)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| {
                        view.toggle_mcp_server(name.clone(), !enabled, cx)
                    }),
                )
                .into_any_element();
            rows = rows.child(self.pref_row(
                server.name.clone(),
                Some(format!("{source} · {}", server.transport.summary())),
                control,
            ));
        }
        // 1 件も無い（＝登録のしかたを案内する）と、絞り込んだ結果 0 件は別の話なので文言を分ける。
        if total == 0 || shown == 0 {
            let message = if total == 0 {
                i18n::t!("settings.mcp_empty")
            } else {
                i18n::t!("settings.mcp_filtered_empty")
            };
            rows = rows.child(
                div()
                    .px(px(12.))
                    .py(px(9.))
                    .rounded(px(8.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(message)),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.section_heading(
                i18n::t!("settings.mcp_heading"),
                Some(i18n::t!("settings.mcp_sub")),
            ))
            // claude.ai のコネクタは**サーバの一覧ではない**（necoder の設定を通らずに入ってくる物を
            // 切る口）ので、下の件数・絞り込み・一覧の外に置く。
            .child(self.toggle_row(
                "claude_ai_connectors",
                0,
                i18n::t!("settings.mcp_claude_connectors"),
                Some(i18n::t!("settings.mcp_claude_connectors_sub")),
                connectors,
                cx,
            ))
            // 件数と絞り込みは 1 件でもある時だけ（空の画面に操作子を並べても読ませるだけ）。
            .when(total > 0, |element| {
                element.child(self.mcp_filter_bar(total, enabled_count, cx))
            })
            .child(rows)
            .child(
                // 注記は fg2（色は識別のためだけに使う・UI-SPEC §1。ここを accent にしない）。
                div()
                    .px(px(12.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.mcp_note"))),
            )
    }

    /// 絞り込み（出所 / 有効のみ）を通る行か。
    fn mcp_row_visible(&self, server: &acp_client::mcp::McpServerConfig) -> bool {
        self.mcp_filter_source
            .is_none_or(|source| source == server.source)
            && (!self.mcp_filter_enabled_only || server.enabled)
    }

    fn set_mcp_filter_source(
        &mut self,
        source: Option<acp_client::mcp::McpSource>,
        cx: &mut Context<Self>,
    ) {
        self.mcp_filter_source = source;
        cx.notify();
    }

    fn toggle_mcp_filter_enabled_only(&mut self, cx: &mut Context<Self>) {
        self.mcp_filter_enabled_only = !self.mcp_filter_enabled_only;
        cx.notify();
    }

    /// 「N 件中 M 件有効」+ 絞り込みチップ。**実際に 1 件以上ある出所だけ**をチップにする
    /// （押しても何も起きないチップを並べない）。
    fn mcp_filter_bar(&self, total: usize, enabled_count: usize, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let mut chips = div().flex().flex_wrap().items_center().gap(px(4.)).child(
            self.filter_chip(
                ("mcp-filter", 0),
                i18n::t!("settings.mcp_filter_all"),
                self.mcp_filter_source.is_none(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _window, cx| view.set_mcp_filter_source(None, cx)),
            ),
        );
        for (index, source) in MCP_SOURCES.iter().enumerate() {
            let source = *source;
            if !self
                .mcp_servers
                .iter()
                .any(|server| server.source == source)
            {
                continue;
            }
            chips = chips.child(
                self.filter_chip(
                    ("mcp-filter", index + 1),
                    mcp_source_label(source),
                    self.mcp_filter_source == Some(source),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _window, cx| {
                        view.set_mcp_filter_source(Some(source), cx)
                    }),
                ),
            );
        }
        chips = chips.child(
            self.filter_chip(
                ("mcp-filter", MCP_SOURCES.len() + 1),
                i18n::t!("settings.mcp_filter_enabled"),
                self.mcp_filter_enabled_only,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _window, cx| view.toggle_mcp_filter_enabled_only(cx)),
            ),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!(
                        "settings.mcp_count",
                        "total" => total,
                        "enabled" => enabled_count,
                    ))),
            )
            .child(chips)
    }

    /// 絞り込みチップ（選択 = accent-dim。外観のテーマチップと同じ書式）。
    fn filter_chip(
        &self,
        id: (&'static str, usize),
        label: String,
        selected: bool,
    ) -> Stateful<Div> {
        let theme = self.theme.clone();
        let accent = self.accent;
        div()
            .id(id)
            .px(px(9.))
            .py(px(3.))
            .rounded(px(5.))
            .text_size(px(11.5))
            .cursor_pointer()
            .when(selected, |element| {
                element.bg(accent.alpha(0.16)).text_color(accent)
            })
            .when(!selected, |element| {
                element
                    .text_color(theme.fg2)
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
            })
            .child(SharedString::from(label))
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
            // プレビュータブ（O26）。値は settings.json の `preview_tabs`。
            .child(self.toggle_row(
                "preview_tabs",
                8,
                i18n::t!("settings.pref_preview_tabs"),
                Some(i18n::t!("settings.pref_preview_tabs_sub")),
                settings.preview_tabs,
                cx,
            ))
            // 自動保存（O26）。値は settings.json の `auto_save`。
            .child(self.segmented_row_with(
                "auto_save",
                i18n::t!("settings.pref_auto_save"),
                Some(i18n::t!("settings.pref_auto_save_sub")),
                &[
                    ("off", i18n::t!("settings.auto_save_off")),
                    ("focus_change", i18n::t!("settings.auto_save_focus_change")),
                    ("after_delay", i18n::t!("settings.auto_save_after_delay")),
                ],
                &settings.auto_save,
                false,
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
                "agent_prewarm",
                6,
                i18n::t!("settings.pref_agent_prewarm"),
                Some(i18n::t!("settings.pref_agent_prewarm_sub")),
                settings.agent_prewarm,
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
            .child(self.segmented_row_with(
                "sound_done",
                i18n::t!("settings.pref_sound_done"),
                None,
                &sound_options(),
                &settings.sound_done,
                true,
                cx,
            ))
            .child(self.segmented_row_with(
                "sound_waiting",
                i18n::t!("settings.pref_sound_waiting"),
                None,
                &sound_options(),
                &settings.sound_waiting,
                true,
                cx,
            ))
            // 通知音の大きさ（O13）。0 は鳴らさない。
            .child(self.stepper_row_with_sub(
                "sound_volume",
                i18n::t!("settings.pref_sound_volume"),
                i18n::t!("settings.pref_sound_volume_sub"),
                settings.sound_volume as f64,
                0.0,
                100.0,
                10.0,
                cx,
            ))
            .child(self.toggle_row(
                "system_notifications",
                7,
                i18n::t!("settings.pref_system_notifications"),
                Some(i18n::t!("settings.pref_system_notifications_sub")),
                settings.system_notifications,
                cx,
            ))
            // ⌘Q・最後の窓を閉じる時の確認（O4）。値は settings.json の `confirm_quit`。
            .child(self.segmented_row_with(
                "confirm_quit",
                i18n::t!("settings.pref_confirm_quit"),
                Some(i18n::t!("settings.pref_confirm_quit_sub")),
                &[
                    ("running", i18n::t!("settings.confirm_quit_running")),
                    ("never", i18n::t!("settings.confirm_quit_never")),
                ],
                &settings.confirm_quit,
                false,
                cx,
            ))
            // エージェントの作業中はスリープさせない（O13）。値は settings.json の `keep_awake`。
            // 止める手段を持つ OS（mac の caffeinate / Windows の SetThreadExecutionState）だけに出す。
            .when(
                cfg!(any(target_os = "macos", target_os = "windows")),
                |group| {
                    group.child(self.segmented_row_with(
                        "keep_awake",
                        i18n::t!("settings.pref_keep_awake"),
                        Some(i18n::t!("settings.pref_keep_awake_sub")),
                        &[
                            ("working", i18n::t!("settings.keep_awake_working")),
                            ("off", i18n::t!("settings.keep_awake_off")),
                        ],
                        &settings.keep_awake,
                        false,
                        cx,
                    ))
                },
            )
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
            .child(self.segmented_row(
                "work_tabs_position",
                i18n::t!("work.tabs_setting"),
                &[
                    ("top", i18n::t!("work.top")),
                    ("left", i18n::t!("work.left")),
                ],
                &settings.work_tabs_position,
                cx,
            ))
            .child(self.stepper_row_with_sub(
                "agent_idle_stop_minutes",
                i18n::t!("settings.pref_agent_idle_stop"),
                i18n::t!("settings.pref_agent_idle_stop_sub"),
                settings.agent_idle_stop_minutes as f64,
                0.0,
                240.0,
                5.0,
                cx,
            ))
            // `ne terminal send`（CLI から端末へ打つ）の許可。既定 off（送った文字はそのまま実行される）。
            // `ne` の節はシムを置けない OS で隠れるが、送信は IPC なので OS を問わずここに置く。
            .child(self.toggle_row(
                "allow_terminal_send",
                7,
                i18n::t!("settings.pref_allow_terminal_send"),
                Some(i18n::t!("settings.pref_allow_terminal_send_sub")),
                settings.allow_terminal_send,
                cx,
            ))
            .child(self.chat_group(settings, cx))
    }

    /// 分数のステッパー（副題つき）。`stepper_row` は副題を持てないので、説明が要る物だけこちら。
    #[allow(clippy::too_many_arguments)]
    fn stepper_row_with_sub(
        &self,
        key: &'static str,
        label: String,
        sub: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        cx: &mut Context<Self>,
    ) -> Div {
        let control = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(self.stepper_button(key, (key, 0), "−", (value - step).max(min), true, cx))
            .child(
                div()
                    .min_w(px(28.))
                    .text_size(px(12.5))
                    .text_color(self.theme.fg0)
                    .child(SharedString::from(format!("{}", value as i64))),
            )
            .child(self.stepper_button(key, (key, 1), "+", (value + step).min(max), true, cx))
            .into_any_element();
        self.pref_row(label, Some(sub), control)
    }

    /// Chat モードの設定（`docs/CHAT.md` §2.2 / §4.1）: 置き場・自動停止・カスタム指示。
    fn chat_group(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let small_button = |id: &'static str, label: String| {
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
                .child(SharedString::from(label))
        };
        let directory = settings.chat.directory.trim().to_string();
        let directory_control = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                small_button(
                    "chat-directory-pick",
                    i18n::t!("settings.chat_directory_pick"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, _, _window, cx| view.pick_chat_directory(cx)),
                ),
            )
            .when(!directory.is_empty(), |row| {
                row.child(
                    small_button(
                        "chat-directory-reset",
                        i18n::t!("settings.chat_directory_reset"),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _window, cx| {
                            let result = set_nested_user_value(
                                cx,
                                "chat",
                                "directory",
                                serde_json::Value::String(String::new()),
                            );
                            view.report_save(result, cx);
                            cx.notify();
                        }),
                    ),
                )
            })
            .into_any_element();
        let idle = settings.chat.idle_stop_minutes as f64;
        let idle_button = |id: (&'static str, usize), glyph: &'static str, target: f64| {
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
                        let result = set_nested_user_value(
                            cx,
                            "chat",
                            "idle_stop_minutes",
                            serde_json::json!(target as i64),
                        );
                        view.report_save(result, cx);
                        cx.notify();
                    }),
                )
        };
        let idle_control = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(idle_button(("chat-idle", 0), "−", (idle - 5.0).max(0.0)))
            .child(
                div()
                    .min_w(px(28.))
                    .text_size(px(12.5))
                    .text_color(theme.fg0)
                    .child(SharedString::from(format!("{}", idle as i64))),
            )
            .child(idle_button(("chat-idle", 1), "+", (idle + 5.0).min(240.0)))
            .into_any_element();
        let instructions = settings.chat.instructions.trim();
        let instructions_sub = if instructions.is_empty() {
            i18n::t!("settings.chat_instructions_none")
        } else {
            let head: String = instructions.chars().take(60).collect();
            if instructions.chars().count() > 60 {
                format!("{head}…")
            } else {
                head
            }
        };
        let instructions_control = small_button(
            "chat-instructions-edit",
            i18n::t!("settings.open_json_button"),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _window, cx| cx.emit(SettingsViewEvent::OpenSettingsJson)),
        )
        .into_any_element();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(14.))
            .child(
                div()
                    .px(px(2.))
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("settings.chat_heading"))),
            )
            .child(self.pref_row(
                i18n::t!("settings.chat_directory"),
                Some(if directory.is_empty() {
                    i18n::t!("settings.chat_directory_default")
                } else {
                    directory.clone()
                }),
                directory_control,
            ))
            .child(self.pref_row(
                i18n::t!("settings.chat_idle_stop"),
                Some(i18n::t!("settings.chat_idle_stop_sub")),
                idle_control,
            ))
            .child(self.pref_row(
                i18n::t!("settings.chat_instructions"),
                Some(instructions_sub),
                instructions_control,
            ))
    }

    /// チャットの置き場をフォルダ選択ダイアログで決める。既にあるチャットのフォルダは動かさない
    /// （置き場が変わるのは、これから作るチャットだけ）。
    fn pick_chat_directory(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(i18n::t!("settings.chat_directory_pick"))),
        });
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = view.update(cx, |view, cx| {
                        let result = set_nested_user_value(
                            cx,
                            "chat",
                            "directory",
                            serde_json::Value::String(path.display().to_string()),
                        );
                        view.report_save(result, cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

/// 通知音のセグメント（同梱の猫の声 → システム音 → オフ）。
/// 完了 / 入力待ちの 2 行で同じ並びを使う＝場面ごとに別の声を当てられる。
/// ラベルの無い声は id をそのまま出す（声を増やしたとき、文言待ちで選べなくならないように）。
/// Captain ボタンを押した後の `captain_agent` の値。任命中の同じエージェント = 解任（`null`）、
/// それ以外 = そのエージェントへ任命（別の Captain からの交代も 1 押し）。
fn next_captain_value(label: &str, current: Option<&str>) -> serde_json::Value {
    if current == Some(label) {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(label.to_string())
    }
}

fn sound_options() -> Vec<(&'static str, String)> {
    let mut options: Vec<(&'static str, String)> = settings_core::SOUND_VOICES
        .iter()
        .map(|voice| {
            let label = match *voice {
                "nya" => i18n::t!("settings.sound_nya"),
                "nyaan" => i18n::t!("settings.sound_nyaan"),
                "mew" => i18n::t!("settings.sound_mew"),
                other => other.to_string(),
            };
            (*voice, label)
        })
        .collect();
    options.push(("system", i18n::t!("settings.sound_system")));
    options.push(("off", i18n::t!("settings.sound_off")));
    options
}

impl EventEmitter<SettingsViewEvent> for SettingsView {}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 開いた時に予約された vendor CLI 調査を、実際に描かれたこの瞬間に 1 回だけ蹴る。
        if self.availability_pending {
            self.probe_availability(cx);
        }
        #[cfg(feature = "remote-preview")]
        if self.remote_preview_only {
            return div()
                .id("remote-preview")
                .size_full()
                .font_family("IBM Plex Sans JP")
                .text_size(px(12.5))
                .bg(self.theme.bg1)
                .p(px(28.))
                .child(self.remote_section(cx));
        }
        let theme = self.theme.clone();
        let accent = self.accent;
        let settings = get(cx);
        let onboarding = !settings.onboarded;
        // 初回は器（ナビ）を出さない — 上から順に読ませたいので 1 枚スクロールのまま（UI-SPEC §12）。
        if onboarding {
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
                                .child(SharedString::from(i18n::t!("settings.welcome_title"))),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("settings.welcome_sub"))),
                        ),
                )
                .child(self.section_heading(
                    i18n::t!("settings.agents_heading"),
                    Some(i18n::t!("settings.agents_sub")),
                ))
                .child(self.agents_rows(&settings, false, cx))
                .child(
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
                );
            return div()
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
                );
        }

        // 2 ペイン: 左 = スクロールしないナビ列 / 右 = 選ばれた 1 ページだけのスクロール面。
        // 設定が増えてもナビに 1 行足すだけで済み、縦に伸び続けない（UI-SPEC §12）。
        div()
            .id("settings-pane")
            .size_full()
            .flex()
            .bg(theme.bg1)
            .child(self.nav_column(cx))
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(
                        div().flex().justify_center().px(px(28.)).py(px(24.)).child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(14.))
                                .w_full()
                                .max_w(px(680.))
                                .child(self.page_body(&settings, cx)),
                        ),
                    ),
            )
    }
}

/// 絞り込みチップに並べる出所（**necoder の設定 → 発見の 3 本**の順）。
const MCP_SOURCES: [acp_client::mcp::McpSource; 4] = [
    acp_client::mcp::McpSource::Necoder,
    acp_client::mcp::McpSource::Codex,
    acp_client::mcp::McpSource::ClaudeCode,
    acp_client::mcp::McpSource::Cursor,
];

/// 出所の表示名。キーを literal で並べる（動的に組むと locales の書き忘れに気づけない）。
fn mcp_source_label(source: acp_client::mcp::McpSource) -> String {
    match source {
        acp_client::mcp::McpSource::Necoder => i18n::t!("settings.mcp_source_necoder"),
        acp_client::mcp::McpSource::Codex => i18n::t!("settings.mcp_source_codex"),
        acp_client::mcp::McpSource::ClaudeCode => i18n::t!("settings.mcp_source_claude"),
        acp_client::mcp::McpSource::Cursor => i18n::t!("settings.mcp_source_cursor"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_login_quotes_the_folder_for_the_shell() {
        assert_eq!(
            account_login_command(
                "CLAUDE_CONFIG_DIR",
                Path::new("/Users/me/Library/Application Support/necoder/accounts/claude/account-2"),
                "claude auth login"
            ),
            "CLAUDE_CONFIG_DIR='/Users/me/Library/Application Support/necoder/accounts/claude/account-2' claude auth login"
        );
        assert_eq!(
            account_login_command("CODEX_HOME", Path::new("/tmp/it's"), "codex login"),
            "CODEX_HOME='/tmp/it'\\''s' codex login",
            "単引用符は閉じて逃がしてから開き直す"
        );
    }

    #[test]
    fn captain_button_appoints_switches_and_dismisses() {
        // 未任命 → 任命
        assert_eq!(
            next_captain_value("Claude Code", None),
            serde_json::Value::String("Claude Code".to_string())
        );
        // 別の Captain からの交代
        assert_eq!(
            next_captain_value("Codex", Some("Claude Code")),
            serde_json::Value::String("Codex".to_string())
        );
        // 任命中の同じエージェント → 解任
        assert_eq!(
            next_captain_value("Claude Code", Some("Claude Code")),
            serde_json::Value::Null
        );
    }

    #[test]
    fn necoder_skill_row_offers_update_before_install() {
        use agent_skills::StubState::{Current, Foreign, Missing, Outdated};
        // 古い版が 1 つでもあれば「更新があります」（無い置き場も同じボタンで置く）。
        assert_eq!(
            necoder_skill_action(&[Current, Outdated]),
            NecoderSkillAction::Update
        );
        assert_eq!(
            necoder_skill_action(&[Outdated, Missing]),
            NecoderSkillAction::Update
        );
        assert_eq!(
            necoder_skill_action(&[Missing, Current]),
            NecoderSkillAction::Install
        );
        assert_eq!(
            necoder_skill_action(&[Current, Current]),
            NecoderSkillAction::Installed
        );
        // 人が書いた SKILL.md は設定画面からは上書きしない（注記だけ）。
        assert_eq!(
            necoder_skill_action(&[Foreign]),
            NecoderSkillAction::Nothing
        );
        assert_eq!(
            necoder_skill_action(&[Current, Foreign]),
            NecoderSkillAction::Nothing
        );
        assert_eq!(necoder_skill_action(&[]), NecoderSkillAction::Nothing);
    }

    #[test]
    fn dismissed_captain_reads_back_as_unappointed() {
        // 解任は `null` を書く。読み戻すと未任命（None）になること＝ Fleet が「任命する」に戻る。
        let store = settings_core::SettingsStore::from_json_layers(&[
            settings_core::DEFAULT_SETTINGS_JSON,
            r#"{ "captain_agent": "Claude Code" }"#,
            r#"{ "captain_agent": null }"#,
        ])
        .expect("マージできる");
        assert_eq!(store.settings().captain_agent, None);
    }

    /// 読めない settings.json に書き手が触らなかった時の知らせ: どのファイルか + 理由を出す。
    #[test]
    fn save_failure_names_the_unreadable_file_and_the_reason() {
        let dir = std::env::temp_dir().join(format!(
            "necoder-settings-save-failure-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let broken = "{ \"theme\": \"necoder-light\", }";
        std::fs::write(&path, broken).expect("seed");
        let error = persist_user_value(&path, "submit_on_enter", serde_json::Value::Bool(true))
            .expect_err("壊れたファイルには書かない");
        let message = save_failure_message(&error);
        assert!(message.contains("settings.json"), "{message}");
        assert!(message.contains("trailing comma"), "{message}");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), broken);

        // プロジェクトの設定（色の保存先）は `.necoder/settings.json` と分かる名前で出す。
        let project = dir.join(".necoder").join("settings.json");
        std::fs::create_dir_all(project.parent().expect("親")).expect("mkdir");
        std::fs::write(&project, "{ // メモ\n}").expect("seed");
        let error = persist_user_value(&project, "color", serde_json::json!("#ff0000"))
            .expect_err("壊れたファイルには書かない");
        assert!(save_failure_message(&error).contains(".necoder/settings.json"));

        // 読めないのではなく書けなかった時は別の文（理由つき）。
        let other = anyhow::anyhow!("disk full");
        assert!(save_failure_message(&other).contains("disk full"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
