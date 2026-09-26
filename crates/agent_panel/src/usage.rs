//! usage — 使用量・レート制限（O11）。スレッドのコストの数え方と、エージェントごとの「最後に受け取った」
//! レート制限の置き場（全ウィンドウの statusbar とポップオーバーが読む）。
//!
//! **値はエージェントが知らせてきた時にしか変わらない。** necoder は利用上限を問い合わせない
//! （資格情報に触れない・常駐のタイマーを持たない）。例外は Codex だけで、ACP に出さない代わりに、
//! 利用者が使用量のポップオーバーを開いた時に `codex app-server` へ 1 回だけ訊く
//! （[`refresh_codex_limits`]・認証は codex 本体）。

pub use acp_client::usage::{LimitStatus, LimitWindow};
use acp_client::usage::{RateLimits, TurnTokens};
use gpui::{App, SharedString};
use std::collections::BTreeMap;

/// これ以上の使用率（表示の整数 %）の窓は目立たせる。
pub const NEAR_LIMIT_PERCENT: f64 = 80.0;

/// Codex を読み直すまでの間（ポップオーバーを開き直すたびにプロセスを立てない）。
const CODEX_REFRESH_INTERVAL_MS: i64 = 60_000;

/// 表示の整数 % が [`NEAR_LIMIT_PERCENT`] 以上か（見えている数字と判定を食い違わせない）。
pub fn is_near_limit(used_percent: f64) -> bool {
    used_percent.round() >= NEAR_LIMIT_PERCENT
}

// ── スレッドのコスト ──

/// スレッド 1 本ぶんの「ターンのコスト」の数え方（O11）。
///
/// Claude（claude-agent-acp）はターンの結果の `usage_update.cost` に**会話の累計**（Claude Code の推定・
/// USD）を載せる。ターンの分はその差分。差分の基準（前回の累計）は、エージェントのプロセスを立て直しても
/// スレッドに残す — `session/load` で引き継いだ会話の最初の累計は、トランスクリプトに残る過去の分を
/// 含むから。再起動後は台帳（`turn_usage.session_cost_usd`）の最後の値を基準にする。基準が分からない
/// 最初の 1 回は、差分を数えずに基準にだけ使う（過去の全額を 1 ターンに載せない）。
#[derive(Debug, Clone, Default)]
pub(crate) struct CostMeter {
    /// エージェントが最後に報告した会話の累計（USD）。`None` = 基準が分からない。
    total_usd: Option<f64>,
    /// まだ台帳に書いていない分（USD）。`None` = コストの報告が無かった。
    pending_usd: Option<f64>,
    /// ターンの終わりに届いたトークン（`PromptResponse._meta.quota`）。
    pending_tokens: Option<TurnTokens>,
}

/// 台帳へ書く 1 ターン分（[`CostMeter::take_turn`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TurnSpend {
    pub tokens: TurnTokens,
    pub cost_usd: Option<f64>,
    pub session_total_usd: Option<f64>,
}

impl CostMeter {
    /// 新しい会話が始まった（`session/new`）。累計は 0 から数え直しになる。
    pub(crate) fn start_fresh(&mut self) {
        self.total_usd = Some(0.0);
    }

    /// 会話を引き継いだ（`session/load`）のに、基準をまだ持っていない（再起動後の最初の再開）。
    pub(crate) fn needs_stored_total(&self) -> bool {
        self.total_usd.is_none()
    }

    /// 台帳に残る前回の累計を基準にする（[`Self::needs_stored_total`] の時だけ呼ぶ）。
    pub(crate) fn resume_from(&mut self, stored_total_usd: Option<f64>) {
        if self.total_usd.is_none() {
            self.total_usd = stored_total_usd;
        }
    }

    /// `usage_update.cost`（会話の累計）を受け取った。
    pub(crate) fn observe_total(&mut self, total_usd: f64) {
        if let Some(previous) = self.total_usd {
            // 減った＝エージェント側で数え直した（`/clear` など）。今の値がそのまま新しい分。
            let spent = if total_usd >= previous - 1e-9 {
                total_usd - previous
            } else {
                total_usd
            };
            *self.pending_usd.get_or_insert(0.0) += spent.max(0.0);
        }
        self.total_usd = Some(total_usd);
    }

    /// ターンの終わりのトークン（`AgentEvent::TurnUsage`）。
    pub(crate) fn observe_tokens(&mut self, tokens: TurnTokens) {
        self.pending_tokens = Some(tokens);
    }

    /// ターンが終わった: 台帳へ書く分を取り出して空にする（トークンもコストも無ければ `None`）。
    pub(crate) fn take_turn(&mut self) -> Option<TurnSpend> {
        if self.pending_tokens.is_none() && self.pending_usd.is_none() {
            return None;
        }
        Some(TurnSpend {
            tokens: self.pending_tokens.take().unwrap_or_default(),
            cost_usd: self.pending_usd.take(),
            session_total_usd: self.total_usd,
        })
    }
}

// ── エージェントごとのレート制限 ──

/// 1 つの窓の、最後に受け取った値。
#[derive(Debug, Clone, PartialEq)]
pub struct WindowReading {
    pub window: LimitWindow,
    /// 使った割合（0〜100）。
    pub used_percent: Option<f64>,
    /// リセットの時刻（unix 秒）。
    pub resets_at: Option<i64>,
    /// この窓の値を受け取った時刻（unix ms）。
    pub received_at_ms: i64,
}

impl WindowReading {
    /// リセットの時刻を過ぎた（値は古い。新しい値はエージェントが次に知らせるまで分からない）。
    pub fn is_reset(&self, now_secs: i64) -> bool {
        self.resets_at.is_some_and(|at| at <= now_secs)
    }
}

/// あるエージェントの、最後に受け取ったレート制限（アカウント単位の値）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentLimits {
    /// 全体の状態（Claude の `status`）。
    pub status: Option<LimitStatus>,
    /// 窓（5 時間 → 週 → その他の順）。
    pub windows: Vec<WindowReading>,
    /// 最後に知らせを受け取った時刻（unix ms）。
    pub received_at_ms: i64,
}

impl AgentLimits {
    /// 1 回分の知らせを重ねる。窓ごとに差し替え、無い値で消さない。リセットの時刻が変わった窓は
    /// 別の期間になったので、使用率も新しい方（無ければ無し）にする。
    fn apply(&mut self, limits: RateLimits, now_ms: i64) {
        if let Some(status) = limits.status {
            self.status = Some(status);
        }
        for incoming in limits.windows {
            match self
                .windows
                .iter_mut()
                .find(|reading| reading.window == incoming.window)
            {
                Some(reading) => {
                    let rolled_over = incoming.resets_at.is_some()
                        && reading.resets_at.is_some()
                        && incoming.resets_at != reading.resets_at;
                    if incoming.used_percent.is_some() || rolled_over {
                        reading.used_percent = incoming.used_percent;
                    }
                    if incoming.resets_at.is_some() {
                        reading.resets_at = incoming.resets_at;
                    }
                    reading.received_at_ms = now_ms;
                }
                None => self.windows.push(WindowReading {
                    window: incoming.window,
                    used_percent: incoming.used_percent,
                    resets_at: incoming.resets_at,
                    received_at_ms: now_ms,
                }),
            }
        }
        self.windows
            .sort_by(|left, right| left.window.cmp(&right.window));
        self.received_at_ms = now_ms;
    }

    /// statusbar に出す窓と使用率: 5 時間枠・週枠と、上限に近いその他の窓。リセット済み・使用率の
    /// 無い窓は出さない。
    pub fn headline(&self, now_secs: i64) -> Vec<(LimitWindow, f64)> {
        self.windows
            .iter()
            .filter(|reading| !reading.is_reset(now_secs))
            .filter_map(|reading| Some((reading.window.clone(), reading.used_percent?)))
            .filter(|(window, percent)| {
                matches!(window, LimitWindow::FiveHour | LimitWindow::Weekly)
                    || is_near_limit(*percent)
            })
            .collect()
    }

    /// 出している窓のどれかが上限に近いか。
    pub fn near_limit(&self, now_secs: i64) -> bool {
        self.headline(now_secs)
            .iter()
            .any(|(_, percent)| is_near_limit(*percent))
    }

    /// 上限に達して止められているか（`rejected` で、上限に達した窓がまだリセット前）。
    pub fn blocked(&self, now_secs: i64) -> bool {
        self.status == Some(LimitStatus::Rejected)
            && self.windows.iter().any(|reading| {
                !reading.is_reset(now_secs)
                    && reading.used_percent.is_some_and(|percent| percent >= 100.0)
            })
    }
}

/// Codex の読み取り（[`refresh_codex_limits`]）の状態。
#[derive(Debug, Clone, Default, PartialEq)]
pub enum CodexRead {
    /// 読んでいない（読み終えた後もここへ戻る）。
    #[default]
    Idle,
    /// `codex app-server` に訊いている。
    Loading,
    /// 読めなかった（codex の文言のまま）。
    Failed(SharedString),
}

/// レート制限の値を分ける鍵（R08）。同じエージェントでも、動かしている場所（手元 / SSH 先）と
/// 認証の置き場（設定の `agent_servers.<id>.env` の `CLAUDE_CONFIG_DIR` / `CODEX_HOME`）が違えば
/// 別のアカウントかもしれないので、値を混ぜない。資格情報そのものは読まない（置き場のパスだけ）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UsageKey {
    pub agent: SharedString,
    /// 動かしている場所（手元は空・SSH 先はホストの表示名）。
    pub host: SharedString,
    /// 認証の置き場（無ければ空 = 既定の置き場）。
    pub profile: SharedString,
}

/// 使用量の鍵の「場所」: 手元は空、SSH 先はホストの表示名（R08）。Host を呼ぶので描画からは使わない。
pub fn host_label(host: &dyn host::Host) -> SharedString {
    if host.is_remote() {
        SharedString::from(host.display_name().to_string())
    } else {
        SharedString::default()
    }
}

/// 認証の置き場を表す env（値はディレクトリのパス。鍵やトークンの env は見ない）。
const AUTH_PROFILE_ENV: [&str; 2] = ["CLAUDE_CONFIG_DIR", "CODEX_HOME"];

impl UsageKey {
    /// 手元・既定の認証の置き場。
    pub fn local(agent: impl Into<SharedString>) -> Self {
        Self {
            agent: agent.into(),
            host: SharedString::default(),
            profile: SharedString::default(),
        }
    }

    /// エージェントを動かしている場所（[`host_label`]）と、設定の認証の置き場から作る。
    /// Host は呼ばない（描画からも呼ばれる）。
    pub fn for_agent(agent: SharedString, host: SharedString, cx: &App) -> Self {
        let profile = acp_client::AGENTS
            .iter()
            .find(|candidate| candidate.label == agent.as_ref())
            .and_then(|candidate| crate::agent_server_override(candidate.id, cx))
            .and_then(|agent_override| {
                AUTH_PROFILE_ENV
                    .iter()
                    .find_map(|key| agent_override.env.get(*key).cloned())
            })
            .map(SharedString::from)
            .unwrap_or_default();
        Self {
            agent,
            host,
            profile,
        }
    }

    /// 見出し: `Claude Code` / `Claude Code · dev-box` / `Claude Code · ~/.claude-work`。
    pub fn label(&self) -> SharedString {
        let mut label = self.agent.to_string();
        for part in [&self.host, &self.profile] {
            if !part.is_empty() {
                label.push_str(" · ");
                label.push_str(part);
            }
        }
        SharedString::from(label)
    }

    /// 手元・既定の置き場の Codex（`refresh_codex_limits` が読みに行くのはこれだけ）か。
    pub fn is_local_codex(&self) -> bool {
        self.agent.as_ref() == codex_label() && self.host.is_empty()
    }
}

impl From<SharedString> for UsageKey {
    fn from(agent: SharedString) -> Self {
        Self::local(agent)
    }
}

impl From<&str> for UsageKey {
    fn from(agent: &str) -> Self {
        Self::local(SharedString::from(agent.to_string()))
    }
}

/// エージェントごと（[`UsageKey`] = エージェント + 動かしている場所 + 認証の置き場）の、最後に受け取った
/// レート制限（O11・R08・gpui Global）。
///
/// アカウント単位の値なので、同じ鍵ならスレッド・プロセス・ウィンドウをまたいで 1 つを共有する。
/// 保存しない（古い値を再起動後に出さない。届くまで statusbar には何も出さない）。
/// **読むときは `cx.try_global`**: `default_global` は観測者（statusbar の再描画）を起こす。
#[derive(Debug, Default)]
pub struct UsageLimits {
    agents: BTreeMap<UsageKey, AgentLimits>,
    /// Codex の読み取りの状態（ポップオーバーに出す）。
    pub codex: CodexRead,
    /// codex が PATH に在るか（ポップオーバーを開いた時に確かめる。`None` = まだ見ていない）。
    pub codex_installed: Option<bool>,
    /// 最後に Codex を読みに行った時刻（unix ms）。
    codex_attempted_at_ms: i64,
}

impl gpui::Global for UsageLimits {}

impl UsageLimits {
    /// エージェントの知らせを重ねる。
    pub fn record(&mut self, key: impl Into<UsageKey>, limits: RateLimits, now_ms: i64) {
        self.agents
            .entry(key.into())
            .or_default()
            .apply(limits, now_ms);
    }

    pub fn get(&self, key: &UsageKey) -> Option<&AgentLimits> {
        self.agents.get(key)
    }

    /// 手元・既定の認証の置き場のエージェントの値。
    pub fn agent(&self, agent: &str) -> Option<&AgentLimits> {
        self.get(&UsageKey::from(agent))
    }

    /// 値のある鍵（エージェント名 → 場所 → 置き場の順）。
    pub fn agents(&self) -> impl Iterator<Item = (&UsageKey, &AgentLimits)> {
        self.agents.iter()
    }
}

/// Codex のラベル（`AgentKind` の表示名。スレッドの `agent` と同じ綴り）。
pub fn codex_label() -> &'static str {
    acp_client::AGENTS
        .iter()
        .find(|agent| agent.id == "codex")
        .map(|agent| agent.label)
        .unwrap_or("Codex")
}

/// Codex の 5 時間枠・週枠を `codex app-server` に訊く（**使用量のポップオーバーを開いた時だけ**・O11）。
///
/// codex が PATH に無ければ何もしない。読んでいる間と、`force` でなければ直近
/// [`CODEX_REFRESH_INTERVAL_MS`] 以内に読みに行った後は重ねない。常駐も定期実行もしない。
/// `settings.json` の `agent_servers.codex.env`（`CODEX_HOME` など）はエージェントと同じものを渡す
/// （同じアカウントを見る）。
pub fn refresh_codex_limits(force: bool, cx: &mut App) {
    // 表示の検証（`NECODER_USAGE_PROBE`）では本物の codex を起こさない（本人のアカウントで外へ出る）。
    #[cfg(debug_assertions)]
    if std::env::var_os("NECODER_USAGE_PROBE").is_some() {
        return;
    }
    // 使わないと決めた Codex（O16）は起こさない（未導入と同じ扱い＝使用量の行も出さない）。
    let codex =
        acp_client::find_in_path("codex").filter(|_| settings::get(cx).agent_enabled("codex"));
    let now = crate::now_unix_ms();
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(codex.is_some());
    let Some(codex) = codex else {
        return;
    };
    if limits.codex == CodexRead::Loading
        || (!force && now - limits.codex_attempted_at_ms < CODEX_REFRESH_INTERVAL_MS)
    {
        return;
    }
    limits.codex = CodexRead::Loading;
    limits.codex_attempted_at_ms = now;
    let env = crate::agent_server_override("codex", cx)
        .map(|agent_override| agent_override.env)
        .unwrap_or_default();
    let read = cx
        .background_executor()
        .spawn(async move { acp_client::codex_limits::read_codex_rate_limits(&codex, &env) });
    cx.spawn(async move |cx| {
        let result = read.await;
        cx.update(|cx| {
            // 読みに行ったのは手元の codex（設定の CODEX_HOME を渡した）。
            let key = UsageKey::for_agent(
                SharedString::from(codex_label()),
                SharedString::default(),
                cx,
            );
            let limits = cx.default_global::<UsageLimits>();
            match result {
                Ok(snapshot) => {
                    limits.record(key, snapshot, crate::now_unix_ms());
                    limits.codex = CodexRead::Idle;
                }
                Err(error) => {
                    eprintln!("Codex の利用上限を読めない: {error:#}");
                    limits.codex = CodexRead::Failed(SharedString::from(format!("{error:#}")));
                }
            }
        });
    })
    .detach();
}

/// 開発用: Codex の値を置き場へ直接入れる（`NECODER_USAGE_PROBE`・O11 の offscreen 検証）。
/// **codex app-server は起こさない**。受け取った時刻は 3 分前にする（「受信 N 分前」を写すため）。
#[cfg(debug_assertions)]
pub fn debug_seed_codex_limits(cx: &mut App) {
    use acp_client::usage::WindowUsage;
    let now_ms = crate::now_unix_ms();
    let now_secs = now_ms / 1000;
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(true);
    limits.record(
        SharedString::from(codex_label()),
        RateLimits {
            status: None,
            windows: vec![
                WindowUsage {
                    window: LimitWindow::FiveHour,
                    used_percent: Some(12.0),
                    resets_at: Some(now_secs + 3 * 3_600 + 40 * 60),
                },
                WindowUsage {
                    window: LimitWindow::Weekly,
                    used_percent: Some(30.0),
                    resets_at: Some(now_secs + 6 * 86_400),
                },
            ],
        },
        now_ms - 3 * 60_000,
    );
}

/// 開発用: Codex の読み取りの状態だけを置く（`NECODER_USAGE_PROBE`・読み込み中 / 失敗の行の描画検証）。
#[cfg(debug_assertions)]
pub fn debug_set_codex_read(read: CodexRead, cx: &mut App) {
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(true);
    limits.codex = read;
}

// ── 表示 ──

/// 窓の名前（ポップオーバー用・長い形）。知らない名前の窓はエージェントの綴りのまま出す。
pub fn window_label(window: &LimitWindow) -> SharedString {
    SharedString::from(match window {
        LimitWindow::FiveHour => i18n::t!("usage.window_five_hour"),
        LimitWindow::Weekly => i18n::t!("usage.window_weekly"),
        LimitWindow::Named(name) => match name.as_str() {
            "seven_day_opus" => i18n::t!("usage.window_opus"),
            "seven_day_sonnet" => i18n::t!("usage.window_sonnet"),
            "seven_day_overage_included" => i18n::t!("usage.window_weekly_extra"),
            "overage" => i18n::t!("usage.window_extra"),
            other => other.to_string(),
        },
        LimitWindow::Minutes(minutes) => match split_duration(*minutes) {
            Duration::Days(days) => i18n::t!("usage.window_days", "days" => days),
            Duration::Hours(hours) => i18n::t!("usage.window_hours", "hours" => hours),
            Duration::Minutes(minutes) => {
                i18n::t!("usage.window_minutes", "minutes" => minutes)
            }
        },
    })
}

/// 窓の名前（statusbar 用・短い形。例 `5h` / `週`）。
pub fn window_short_label(window: &LimitWindow) -> SharedString {
    SharedString::from(match window {
        LimitWindow::FiveHour => i18n::t!("usage.window_five_hour_short"),
        LimitWindow::Weekly => i18n::t!("usage.window_weekly_short"),
        LimitWindow::Named(name) => match name.as_str() {
            "seven_day_opus" => i18n::t!("usage.window_opus_short"),
            "seven_day_sonnet" => i18n::t!("usage.window_sonnet_short"),
            "seven_day_overage_included" => i18n::t!("usage.window_weekly_extra_short"),
            "overage" => i18n::t!("usage.window_extra_short"),
            other => other.to_string(),
        },
        LimitWindow::Minutes(minutes) => match split_duration(*minutes) {
            Duration::Days(days) => i18n::t!("usage.window_days_short", "days" => days),
            Duration::Hours(hours) => i18n::t!("usage.window_hours_short", "hours" => hours),
            Duration::Minutes(minutes) => {
                i18n::t!("usage.window_minutes_short", "minutes" => minutes)
            }
        },
    })
}

/// 窓の長さを割り切れる単位で（日 → 時間 → 分）。
enum Duration {
    Days(u64),
    Hours(u64),
    Minutes(u64),
}

fn split_duration(minutes: u64) -> Duration {
    if minutes > 0 && minutes % 1_440 == 0 {
        Duration::Days(minutes / 1_440)
    } else if minutes > 0 && minutes % 60 == 0 {
        Duration::Hours(minutes / 60)
    } else {
        Duration::Minutes(minutes)
    }
}

/// 使用率の表示（整数 %）。
pub fn percent_label(used_percent: f64) -> String {
    format!("{:.0}%", used_percent)
}

/// statusbar の 1 行（例 `5h 42% · 週 18%`）。
pub fn headline_label(headline: &[(LimitWindow, f64)]) -> SharedString {
    SharedString::from(
        headline
            .iter()
            .map(|(window, percent)| {
                format!("{} {}", window_short_label(window), percent_label(*percent))
            })
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

/// リセットまでの時間（例「あと 2 時間 13 分でリセット」）。過ぎていれば「リセット済み」。
pub fn resets_in_label(resets_at: i64, now_secs: i64) -> SharedString {
    let remaining = resets_at - now_secs;
    if remaining <= 0 {
        return SharedString::from(i18n::t!("usage.reset_done"));
    }
    // 分は切り上げ（「あと 0 分」と出さない）。
    let minutes = (remaining + 59) / 60;
    // 端数が 0 の単位は言わない（「6 日 0 時間」にしない）。
    let (days, hours) = (minutes / (24 * 60), minutes % (24 * 60) / 60);
    let time = if minutes < 60 {
        i18n::t!("usage.duration_minutes", "minutes" => minutes)
    } else if minutes < 24 * 60 && minutes % 60 == 0 {
        i18n::t!("usage.duration_hours_only", "hours" => minutes / 60)
    } else if minutes < 24 * 60 {
        i18n::t!("usage.duration_hours", "hours" => minutes / 60, "minutes" => minutes % 60)
    } else if hours == 0 {
        i18n::t!("usage.duration_days_only", "days" => days)
    } else {
        i18n::t!("usage.duration_days", "days" => days, "hours" => hours)
    };
    SharedString::from(i18n::t!("usage.resets_in", "time" => time))
}

/// トークン数（例 `850` / `23.4k` / `1.25M`）。
pub fn tokens_label(tokens: i64) -> String {
    let tokens = tokens.max(0) as f64;
    if tokens < 1_000.0 {
        format!("{tokens:.0}")
    } else if tokens < 1_000_000.0 {
        format!("{:.1}k", tokens / 1_000.0)
    } else {
        format!("{:.2}M", tokens / 1_000_000.0)
    }
}

/// 金額（USD。例 `$0.37`・1 セント未満は `<$0.01`）。
pub fn usd_label(amount: f64) -> String {
    if amount > 0.0 && amount < 0.005 {
        "<$0.01".to_string()
    } else {
        format!("${:.2}", amount.max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R08: 同じエージェントでも、SSH 先・認証の置き場が違えば値を混ぜない。
    #[test]
    fn limits_are_kept_apart_per_host_and_profile() {
        use acp_client::usage::WindowUsage;
        let window = |percent: f64| RateLimits {
            status: None,
            windows: vec![WindowUsage {
                window: LimitWindow::FiveHour,
                used_percent: Some(percent),
                resets_at: Some(9_999_999_999),
            }],
        };
        let mut limits = UsageLimits::default();
        let local = UsageKey::local("Claude Code");
        let remote = UsageKey {
            host: "dev-box".into(),
            ..UsageKey::local("Claude Code")
        };
        let work = UsageKey {
            profile: "~/.claude-work".into(),
            ..UsageKey::local("Claude Code")
        };
        limits.record(local.clone(), window(10.0), 1);
        limits.record(remote.clone(), window(90.0), 2);
        limits.record(work.clone(), window(50.0), 3);
        let percent = |key: &UsageKey| {
            limits.get(key).expect("値がある").windows[0]
                .used_percent
                .expect("使用率")
        };
        assert_eq!(percent(&local), 10.0);
        assert_eq!(percent(&remote), 90.0, "SSH 先の値は手元と混ぜない");
        assert_eq!(percent(&work), 50.0, "別の認証の置き場も混ぜない");
        assert_eq!(limits.agents().count(), 3);
        assert_eq!(remote.label().as_ref(), "Claude Code · dev-box");
        assert_eq!(work.label().as_ref(), "Claude Code · ~/.claude-work");
        assert_eq!(
            limits
                .agent("Claude Code")
                .map(|limits| limits.received_at_ms),
            Some(1),
            "名前だけで引くのは手元・既定の置き場"
        );
    }
    use acp_client::usage::WindowUsage;

    fn window(window: LimitWindow, percent: Option<f64>, resets_at: Option<i64>) -> WindowUsage {
        WindowUsage {
            window,
            used_percent: percent,
            resets_at,
        }
    }

    /// ターンのコスト = 会話の累計の差分。基準はプロセスをまたいで残し、減ったら数え直し（`/clear`）、
    /// 基準が分からない最初の 1 回は数えずに基準にだけ使う。
    #[test]
    fn turn_cost_is_the_difference_of_the_running_total() {
        let mut meter = CostMeter::default();
        meter.start_fresh();
        meter.observe_total(0.25);
        meter.observe_tokens(TurnTokens {
            total: 1_000,
            ..TurnTokens::default()
        });
        let first = meter.take_turn().expect("1 ターン目");
        assert_eq!(first.tokens.total, 1_000);
        assert!((first.cost_usd.expect("コスト") - 0.25).abs() < 1e-9);
        assert_eq!(first.session_total_usd, Some(0.25));
        assert_eq!(meter.take_turn(), None, "書いたら空になる");

        // 同じ会話の次のターン（エージェントを立て直して引き継いでも基準は残る）。
        assert!(!meter.needs_stored_total());
        meter.observe_total(0.75);
        let second = meter.take_turn().expect("2 ターン目");
        assert!((second.cost_usd.expect("コスト") - 0.5).abs() < 1e-9);
        assert_eq!(
            second.tokens,
            TurnTokens::default(),
            "トークンの報告が無いターン"
        );

        // 累計が減った＝数え直し。今の値がそのまま新しい分。
        meter.observe_total(0.1);
        assert!((meter.take_turn().expect("3").cost_usd.expect("コスト") - 0.1).abs() < 1e-9);

        // 再起動後に引き継いだ会話: 台帳の累計が基準。
        let mut resumed = CostMeter::default();
        assert!(resumed.needs_stored_total());
        resumed.resume_from(Some(5.0));
        resumed.observe_total(5.4);
        assert!((resumed.take_turn().expect("再開").cost_usd.expect("コスト") - 0.4).abs() < 1e-9);

        // 基準が分からない（台帳にも無い）最初の値は数えない。次からは数える。
        let mut unknown = CostMeter::default();
        unknown.resume_from(None);
        unknown.observe_total(12.0);
        assert_eq!(unknown.take_turn(), None, "過去の全額を 1 ターンに載せない");
        unknown.observe_total(12.3);
        assert!((unknown.take_turn().expect("次").cost_usd.expect("コスト") - 0.3).abs() < 1e-9);
    }

    /// 80% の判定は表示の整数 % で行う（見えている数字と食い違わない）。
    #[test]
    fn near_limit_follows_the_displayed_percent() {
        assert!(!is_near_limit(79.4));
        assert!(is_near_limit(79.6));
        assert!(is_near_limit(80.0));
        assert!(is_near_limit(100.0));
        assert_eq!(percent_label(79.6), "80%");
    }

    /// 窓ごとに差し替える。statusbar は 5 時間・週と、上限に近いその他の窓だけ。リセット済みは出さない。
    #[test]
    fn limits_merge_per_window_and_pick_the_headline() {
        let mut limits = UsageLimits::default();
        limits.record(
            "Claude Code",
            RateLimits {
                status: Some(LimitStatus::Allowed),
                windows: vec![
                    window(LimitWindow::Weekly, Some(18.0), Some(2_000)),
                    window(LimitWindow::FiveHour, Some(42.0), Some(1_000)),
                    window(
                        LimitWindow::Named("seven_day_opus".into()),
                        Some(30.0),
                        Some(2_000),
                    ),
                ],
            },
            10,
        );
        // 代表の窓だけの知らせ（5 時間枠）は、他の窓を消さない。
        limits.record(
            "Claude Code",
            RateLimits {
                status: None,
                windows: vec![window(LimitWindow::FiveHour, Some(44.0), None)],
            },
            20,
        );
        let claude = limits.agent("Claude Code").expect("値がある");
        assert_eq!(
            claude.headline(500),
            vec![(LimitWindow::FiveHour, 44.0), (LimitWindow::Weekly, 18.0)],
            "5 時間 → 週の順・Opus の週枠は 80% 未満なので出さない"
        );
        assert_eq!(claude.windows[0].received_at_ms, 20);
        assert_eq!(
            claude.windows[1].received_at_ms, 10,
            "受け取った時刻は窓ごと"
        );
        assert!(!claude.near_limit(500));
        assert_eq!(
            headline_label(&claude.headline(500)).as_ref(),
            "5h 44% · 7d 18%"
        );

        // その他の窓も 80% を超えれば出す。5 時間枠がリセット済みなら消える。
        limits.record(
            "Claude Code",
            RateLimits {
                status: Some(LimitStatus::Warning),
                windows: vec![window(
                    LimitWindow::Named("seven_day_opus".into()),
                    Some(91.0),
                    Some(2_000),
                )],
            },
            30,
        );
        let claude = limits.agent("Claude Code").expect("値がある");
        assert_eq!(
            claude.headline(1_500),
            vec![
                (LimitWindow::Weekly, 18.0),
                (LimitWindow::Named("seven_day_opus".into()), 91.0)
            ]
        );
        assert!(claude.near_limit(1_500));
        assert!(limits.agent("Codex").is_none(), "エージェントごとに別");
    }

    /// 止められた状態は、上限に達した窓がリセットされるまで。リセット時刻が変わった窓は使用率も入れ替える。
    #[test]
    fn a_rejection_lasts_until_the_window_resets() {
        let mut limits = UsageLimits::default();
        limits.record(
            "Claude Code",
            RateLimits {
                status: Some(LimitStatus::Rejected),
                windows: vec![window(LimitWindow::FiveHour, Some(100.0), Some(1_000))],
            },
            10,
        );
        let claude = limits.agent("Claude Code").expect("値がある");
        assert!(claude.blocked(999));
        assert!(
            !claude.blocked(1_000),
            "リセットの時刻を過ぎたら止められていない"
        );
        assert!(
            claude.headline(1_000).is_empty(),
            "リセット済みの値は出さない"
        );

        // 次の期間の知らせ（リセット時刻が変わった・使用率はまだ無い）で古い 100% を残さない。
        limits.record(
            "Claude Code",
            RateLimits {
                status: Some(LimitStatus::Allowed),
                windows: vec![window(LimitWindow::FiveHour, None, Some(19_000))],
            },
            20,
        );
        let claude = limits.agent("Claude Code").expect("値がある");
        assert_eq!(claude.windows[0].used_percent, None);
        assert!(!claude.blocked(1_500));
    }

    /// 表示の組み立て（テストは既定の en ロケール。ロケールは全体の状態なので切り替えない）。
    #[test]
    fn labels_for_windows_durations_and_amounts() {
        assert_eq!(window_short_label(&LimitWindow::Weekly).as_ref(), "7d");
        assert_eq!(
            window_label(&LimitWindow::Named("seven_day_opus".into())).as_ref(),
            "Weekly (Opus)"
        );
        assert_eq!(
            window_label(&LimitWindow::Named("mystery".into())).as_ref(),
            "mystery",
            "知らない名前は綴りのまま"
        );
        assert_eq!(
            window_label(&LimitWindow::Minutes(120)).as_ref(),
            "2-hour window"
        );
        assert_eq!(
            window_short_label(&LimitWindow::Minutes(2_880)).as_ref(),
            "2d"
        );
        assert_eq!(
            window_short_label(&LimitWindow::Minutes(45)).as_ref(),
            "45m"
        );
        assert_eq!(
            resets_in_label(1_000 + 2 * 3_600 + 13 * 60, 1_000).as_ref(),
            "resets in 2h 13m"
        );
        assert_eq!(
            resets_in_label(1_030, 1_000).as_ref(),
            "resets in 1m",
            "分は切り上げ（0 分と出さない）"
        );
        assert_eq!(
            resets_in_label(1_000 + 4 * 86_400 + 3 * 3_600, 1_000).as_ref(),
            "resets in 4d 3h"
        );
        assert_eq!(
            resets_in_label(1_000 + 6 * 86_400 + 30, 1_000).as_ref(),
            "resets in 6d",
            "0 時間は言わない"
        );
        assert_eq!(
            resets_in_label(1_000 + 3 * 3_600, 1_000).as_ref(),
            "resets in 3h"
        );
        assert_eq!(
            resets_in_label(900, 1_000).as_ref(),
            "reset (no new value yet)"
        );
        assert_eq!(tokens_label(850), "850");
        assert_eq!(tokens_label(23_400), "23.4k");
        assert_eq!(tokens_label(1_250_000), "1.25M");
        assert_eq!(usd_label(0.374), "$0.37");
        assert_eq!(usd_label(0.001), "<$0.01");
        assert_eq!(usd_label(0.0), "$0.00");
    }
}
