//! usage — 使用量・レート制限（O11）。スレッドのコストの数え方と、エージェントごとの「最後に受け取った」
//! レート制限の置き場（全ウィンドウの statusbar とポップオーバーが読む）。
//!
//! **値はエージェントが知らせてきた時にしか変わらない。** necoder は利用上限を問い合わせない
//! （資格情報に触れない・常駐のタイマーを持たない）。例外は Codex だけで、ACP に出さない代わりに、
//! 利用者が使用量のポップオーバーを開いた時に `codex app-server` へ 1 回だけ訊く
//! （[`refresh_codex_limits`]・認証は codex 本体）。
//!
//! **値の持ち主はエージェントの名前ではなくアカウント**（[`UsageAccount`]・R08）。同じエージェントでも、
//! 実行する host（手元 / SSH の接続先）と認証の環境（設定 `agent_servers.<id>` の env・起動コマンド）が
//! 違えば別のアカウントになり得るので、値を混ぜない。

pub use acp_client::usage::{LimitStatus, LimitWindow};
use acp_client::usage::{RateLimits, TurnTokens};
use gpui::{App, SharedString};
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};

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

/// 台帳へ書く 1 ターン分（[`CostMeter::take_turn`]）。報告の無い値は `None`（0 にしない・R08）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TurnSpend {
    pub tokens: Option<TurnTokens>,
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
            tokens: self.pending_tokens.take(),
            cost_usd: self.pending_usd.take(),
            session_total_usd: self.total_usd,
        })
    }
}

// ── レート制限の持ち主 ──

/// 認証の置き場を指す env（値はディレクトリのパス＝画面に出してよい）。これがあれば見分けの表示に使う。
const AUTH_HOME_VARIABLES: [&str; 2] = ["CLAUDE_CONFIG_DIR", "CODEX_HOME"];

/// 実行 host の見分け（id と、SSH なら接続先の表示）。宛先が決まった時に 1 回だけ host から取っておく
/// （Render の中から host に触らない約束があるため。チップは描くたびに持ち主を引く）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageHost {
    id: SharedString,
    remote: Option<SharedString>,
}

impl UsageHost {
    pub fn of(host: &dyn host::Host) -> Self {
        Self {
            id: SharedString::from(host.id().to_string()),
            remote: host
                .is_remote()
                .then(|| SharedString::from(host.display_name().to_string())),
        }
    }

    pub fn local() -> Self {
        Self::of(&host::LocalHost)
    }
}

/// レート制限の持ち主（R08）。レート制限はアカウント単位の値だが、necoder はアカウントを直接は
/// 知らない（資格情報を読まない）。そこで「エージェント × 実行 host × 認証の環境」が同じなら同じ
/// アカウントとみなして値を共有し、どれかが違えば別の持ち主として分ける。
///
/// 認証の環境は設定 `agent_servers.<id>` の env と起動コマンドの**指紋**だけを持つ（API キーなどの値を
/// 抱えない）。同じ置き場のままログインし直した（`/login` で別のアカウントにした）場合は見分けられない
/// ので、ポップオーバーには受け取った時刻を必ず添える。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UsageAccount {
    /// エージェントのラベル（`AgentKind` の表示名。スレッドの `agent` と同じ綴り。例 `Claude Code`）。
    pub agent: SharedString,
    /// 実行 host の id（`local` / `ssh://user@host:port`）。
    host: SharedString,
    /// 認証の環境の指紋（env・起動コマンドから）。設定で何も足していなければ 0。
    profile: u64,
    /// 表示用: SSH の接続先（`user@host`）。手元なら `None`。
    remote: Option<SharedString>,
    /// 表示用: 認証の置き場（`CLAUDE_CONFIG_DIR` / `CODEX_HOME` の値）。
    profile_home: Option<SharedString>,
}

impl UsageAccount {
    /// `agent` を `host` の上で、設定 `agent_servers.<id>`（`agent_override`）の env とコマンドで
    /// 動かす時の持ち主。セッションを立てる時に決め、そのセッションの知らせはこれに重ねる。
    pub fn new(
        agent: impl Into<SharedString>,
        host: &UsageHost,
        agent_override: Option<&acp_client::AgentOverride>,
    ) -> Self {
        Self::with_host(agent, &host.id, host.remote.clone(), agent_override)
    }

    fn with_host(
        agent: impl Into<SharedString>,
        host_id: &str,
        remote: Option<SharedString>,
        agent_override: Option<&acp_client::AgentOverride>,
    ) -> Self {
        let profile = agent_override.map(profile_fingerprint).unwrap_or(0);
        let profile_home = agent_override.and_then(|agent_override| {
            AUTH_HOME_VARIABLES
                .iter()
                .find_map(|name| agent_override.env.get(*name))
                .map(|home| SharedString::from(home.clone()))
        });
        Self {
            agent: agent.into(),
            host: SharedString::from(host_id.to_string()),
            profile,
            remote,
            profile_home,
        }
    }

    /// 手元で動かす時の持ち主（Codex の `codex app-server` は手元で訊く）。
    pub fn local(
        agent: impl Into<SharedString>,
        agent_override: Option<&acp_client::AgentOverride>,
    ) -> Self {
        Self::new(agent, &UsageHost::local(), agent_override)
    }

    /// エージェント名の隣に添える見分け（例 `dev@devbox（SSH） · ~/.claude-work`）。手元の既定の環境
    /// なら `None`（名前だけで足りる）。
    pub fn detail(&self) -> Option<SharedString> {
        let mut parts = Vec::new();
        if let Some(remote) = &self.remote {
            parts.push(i18n::t!("usage.account_ssh", "host" => remote));
        }
        match &self.profile_home {
            Some(home) => parts.push(home.to_string()),
            None if self.profile != 0 => parts.push(i18n::t!(
                "usage.account_env",
                "id" => format!("#{:06x}", self.profile & 0xff_ffff)
            )),
            None => {}
        }
        (!parts.is_empty()).then(|| SharedString::from(parts.join(" · ")))
    }

    /// 名前 + 見分け（ツールチップ・ポップオーバーの見出し用）。
    pub fn label(&self) -> SharedString {
        match self.detail() {
            Some(detail) => SharedString::from(format!("{} · {detail}", self.agent)),
            None => self.agent.clone(),
        }
    }
}

/// 設定 `agent_servers.<id>` の指紋。env を足していない・コマンドを差し替えていないなら 0。
/// 値そのものは残さない（API キーを env に書く人もいる）。
fn profile_fingerprint(agent_override: &acp_client::AgentOverride) -> u64 {
    if agent_override.command.is_none()
        && agent_override.args.is_empty()
        && agent_override.env.is_empty()
    {
        return 0;
    }
    let mut hasher = DefaultHasher::new();
    agent_override.command.hash(&mut hasher);
    agent_override.args.hash(&mut hasher);
    agent_override.env.hash(&mut hasher);
    // 0 は「何も足していない」に取っておく。
    hasher.finish().max(1)
}

// ── アカウントごとのレート制限 ──

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

/// あるアカウント（[`UsageAccount`]）の、最後に受け取ったレート制限。
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

/// アカウント（[`UsageAccount`]）ごとの、最後に受け取ったレート制限（O11・gpui Global）。
///
/// アカウント単位の値なので、持ち主が同じならスレッド・プロセス・ウィンドウをまたいで 1 つを共有する。
/// 持ち主が違えば（SSH の接続先・認証の環境が違えば）同じエージェントでも混ぜない（R08）。
/// 保存しない（古い値を再起動後に出さない。届くまで statusbar には何も出さない）。
/// **読むときは `cx.try_global`**: `default_global` は観測者（statusbar の再描画）を起こす。
#[derive(Debug, Default)]
pub struct UsageLimits {
    accounts: BTreeMap<UsageAccount, AgentLimits>,
    /// Codex の読み取りの状態（ポップオーバーに出す）。
    pub codex: CodexRead,
    /// その読み取りが誰の分か（読みに行った時の手元の Codex と認証の環境）。
    pub codex_account: Option<UsageAccount>,
    /// codex が PATH に在るか（ポップオーバーを開いた時に確かめる。`None` = まだ見ていない）。
    pub codex_installed: Option<bool>,
    /// 最後に Codex を読みに行った時刻（unix ms）。
    codex_attempted_at_ms: i64,
}

impl gpui::Global for UsageLimits {}

impl UsageLimits {
    /// あるアカウントの知らせを重ねる。
    pub fn record(&mut self, account: UsageAccount, limits: RateLimits, now_ms: i64) {
        self.accounts
            .entry(account)
            .or_default()
            .apply(limits, now_ms);
    }

    pub fn account(&self, account: &UsageAccount) -> Option<&AgentLimits> {
        self.accounts.get(account)
    }

    /// 値のあるアカウント（エージェント名 → host → 認証の環境の順）。
    pub fn accounts(&self) -> impl Iterator<Item = (&UsageAccount, &AgentLimits)> {
        self.accounts.iter()
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

/// 手元の Codex の持ち主（今の設定 `agent_servers.codex` の認証の環境で。`codex app-server` を訊く先）。
pub fn local_codex_account(cx: &App) -> UsageAccount {
    UsageAccount::local(
        codex_label(),
        crate::agent_server_override("codex", cx).as_ref(),
    )
}

/// Codex の 5 時間枠・週枠を `codex app-server` に訊く（**使用量のポップオーバーを開いた時だけ**・O11）。
///
/// codex が PATH に無ければ何もしない。読んでいる間と、`force` でなければ直近
/// [`CODEX_REFRESH_INTERVAL_MS`] 以内に読みに行った後は重ねない。常駐も定期実行もしない。
/// `settings.json` の `agent_servers.codex.env`（`CODEX_HOME` など）はエージェントと同じものを渡す
/// （同じアカウントを見る）。値は「手元の Codex × その認証の環境」の持ち主へ重ねる（SSH 先の Codex の
/// スレッドとは混ぜない・R08）。
pub fn refresh_codex_limits(force: bool, cx: &mut App) {
    // 表示の検証（`NECODER_USAGE_PROBE`）では本物の codex を起こさない（本人のアカウントで外へ出る）。
    #[cfg(debug_assertions)]
    if std::env::var_os("NECODER_USAGE_PROBE").is_some() {
        return;
    }
    let codex = acp_client::find_in_path("codex");
    let now = crate::now_unix_ms();
    let account = local_codex_account(cx);
    let env = crate::agent_server_override("codex", cx)
        .map(|agent_override| agent_override.env)
        .unwrap_or_default();
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(codex.is_some());
    let Some(codex) = codex else {
        return;
    };
    // 認証の環境を変えた後は、前の持ち主の読み取りの間隔を待たない（別のアカウントを読む）。
    let same_account = limits.codex_account.as_ref() == Some(&account);
    if (limits.codex == CodexRead::Loading && same_account)
        || (!force
            && same_account
            && now - limits.codex_attempted_at_ms < CODEX_REFRESH_INTERVAL_MS)
    {
        return;
    }
    limits.codex = CodexRead::Loading;
    limits.codex_account = Some(account.clone());
    limits.codex_attempted_at_ms = now;
    let read = cx
        .background_executor()
        .spawn(async move { acp_client::codex_limits::read_codex_rate_limits(&codex, &env) });
    cx.spawn(async move |cx| {
        let result = read.await;
        cx.update(|cx| {
            let limits = cx.default_global::<UsageLimits>();
            // 読んでいる間に認証の環境を変えて読み直した: 古い読み取りの結果で新しい状態を上書きしない
            // （値は読んだ持ち主の方へ重ねる）。
            let current = limits.codex_account.as_ref() == Some(&account);
            match result {
                Ok(snapshot) => {
                    limits.record(account, snapshot, crate::now_unix_ms());
                    if current {
                        limits.codex = CodexRead::Idle;
                    }
                }
                Err(error) => {
                    eprintln!("Codex の利用上限を読めない: {error:#}");
                    if current {
                        limits.codex = CodexRead::Failed(SharedString::from(format!("{error:#}")));
                    }
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
    let account = local_codex_account(cx);
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(true);
    limits.codex_account = Some(account.clone());
    limits.record(
        account,
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
    let account = local_codex_account(cx);
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(true);
    limits.codex_account = Some(account);
    limits.codex = read;
}

/// 開発用: 同じ Claude Code の別のアカウント（SSH の接続先・別の認証の置き場）の値を置く
/// （`NECODER_USAGE_PROBE=accounts`・R08 の表示の検証）。SSH には繋がない。
#[cfg(debug_assertions)]
pub fn debug_seed_other_accounts(cx: &mut App) {
    use acp_client::usage::WindowUsage;
    let now_ms = crate::now_unix_ms();
    let now_secs = now_ms / 1000;
    let work = acp_client::AgentOverride {
        command: None,
        args: Vec::new(),
        env: BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".to_string(),
            "~/.claude-work".to_string(),
        )]),
    };
    let accounts = [
        (
            UsageAccount::with_host(
                "Claude Code",
                "ssh://dev@devbox",
                Some(SharedString::from("dev@devbox")),
                None,
            ),
            9.0,
            now_ms - 12 * 60_000,
        ),
        (
            UsageAccount::local("Claude Code", Some(&work)),
            67.0,
            now_ms - 40 * 60_000,
        ),
    ];
    let limits = cx.default_global::<UsageLimits>();
    for (account, percent, received_at_ms) in accounts {
        limits.record(
            account,
            RateLimits {
                status: None,
                windows: vec![WindowUsage {
                    window: LimitWindow::FiveHour,
                    used_percent: Some(percent),
                    resets_at: Some(now_secs + 2 * 3_600),
                }],
            },
            received_at_ms,
        );
    }
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

/// 報告から数えた合計の表示。報告が 1 つも無ければ `—`（0 と断定しない）、一部のターンにしか
/// 報告が無ければ `≥`（報告の無い分は足していない＝下限）を付ける（R08）。
pub fn reported_total_label(total: Option<String>, reported_turns: i64, turns: i64) -> String {
    match total {
        None => "—".to_string(),
        Some(total) if reported_turns < turns => format!("≥ {total}"),
        Some(total) => total,
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
    use acp_client::usage::WindowUsage;

    fn claude_account() -> UsageAccount {
        UsageAccount::local("Claude Code", None)
    }

    fn with_env(pairs: &[(&str, &str)]) -> acp_client::AgentOverride {
        acp_client::AgentOverride {
            command: None,
            args: Vec::new(),
            env: pairs
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        }
    }

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
        assert_eq!(first.tokens.map(|tokens| tokens.total), Some(1_000));
        assert!((first.cost_usd.expect("コスト") - 0.25).abs() < 1e-9);
        assert_eq!(first.session_total_usd, Some(0.25));
        assert_eq!(meter.take_turn(), None, "書いたら空になる");

        // 同じ会話の次のターン（エージェントを立て直して引き継いでも基準は残る）。
        assert!(!meter.needs_stored_total());
        meter.observe_total(0.75);
        let second = meter.take_turn().expect("2 ターン目");
        assert!((second.cost_usd.expect("コスト") - 0.5).abs() < 1e-9);
        assert_eq!(
            second.tokens, None,
            "トークンの報告が無いターンは 0 ではなく「無い」"
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

    /// R08: 持ち主は「エージェント × 実行 host × 認証の環境」。同じ 3 つなら同じ（値を共有）、どれかが
    /// 違えば別。認証の環境は指紋だけを持ち、API キーなどの値は抱えない・画面にも出さない。
    #[test]
    fn accounts_are_told_apart_by_host_and_auth_environment() {
        let work = with_env(&[("CLAUDE_CONFIG_DIR", "/Users/me/.claude-work")]);
        let personal = with_env(&[("CLAUDE_CONFIG_DIR", "/Users/me/.claude")]);
        let api_key = with_env(&[("ANTHROPIC_API_KEY", "sk-ant-secret-value")]);
        let remote = UsageAccount::with_host(
            "Claude Code",
            "ssh://dev@devbox",
            Some("dev@devbox".into()),
            None,
        );
        let accounts = [
            claude_account(),
            UsageAccount::local("Claude Code", Some(&work)),
            UsageAccount::local("Claude Code", Some(&personal)),
            UsageAccount::local("Claude Code", Some(&api_key)),
            remote.clone(),
            UsageAccount::local("Codex", None),
        ];
        for (index, left) in accounts.iter().enumerate() {
            for right in &accounts[index + 1..] {
                assert_ne!(left, right, "{left:?} と {right:?} は別の持ち主");
            }
        }
        assert_eq!(
            UsageAccount::local("Claude Code", Some(&work)),
            UsageAccount::local("Claude Code", Some(&work.clone())),
            "同じ host・同じ認証の環境なら同じ持ち主（スレッドをまたいで共有する）"
        );
        assert_eq!(
            UsageAccount::local("Claude Code", Some(&with_env(&[]))),
            claude_account(),
            "env を何も足していなければ既定の環境"
        );

        // 見分けの表示: 手元の既定は名前だけ。SSH は接続先、置き場があればそのパス、無ければ指紋。
        assert_eq!(claude_account().detail(), None);
        assert!(remote
            .detail()
            .is_some_and(|detail| detail.contains("dev@devbox")));
        assert!(UsageAccount::local("Claude Code", Some(&work))
            .detail()
            .is_some_and(|detail| detail.contains("/Users/me/.claude-work")));
        let keyed = UsageAccount::local("Claude Code", Some(&api_key));
        let shown = format!("{:?} {}", keyed, keyed.label());
        assert!(keyed.detail().is_some_and(|detail| detail.contains('#')));
        assert!(!shown.contains("sk-ant-secret-value"), "{shown}");
    }

    /// R08: 同じエージェントでも持ち主が違えば値は混ざらない（片方の知らせで他方は変わらない）。
    #[test]
    fn limits_of_different_accounts_do_not_mix() {
        let work = UsageAccount::local(
            "Claude Code",
            Some(&with_env(&[(
                "CLAUDE_CONFIG_DIR",
                "/Users/me/.claude-work",
            )])),
        );
        let mut limits = UsageLimits::default();
        limits.record(
            claude_account(),
            RateLimits {
                status: Some(LimitStatus::Allowed),
                windows: vec![window(LimitWindow::FiveHour, Some(12.0), Some(5_000))],
            },
            10,
        );
        limits.record(
            work.clone(),
            RateLimits {
                status: Some(LimitStatus::Warning),
                windows: vec![window(LimitWindow::FiveHour, Some(91.0), Some(5_000))],
            },
            20,
        );
        assert_eq!(
            limits
                .account(&claude_account())
                .map(|limits| limits.headline(100)),
            Some(vec![(LimitWindow::FiveHour, 12.0)])
        );
        assert_eq!(
            limits.account(&work).map(|limits| limits.headline(100)),
            Some(vec![(LimitWindow::FiveHour, 91.0)])
        );
        assert!(!limits
            .account(&claude_account())
            .is_some_and(|limits| limits.near_limit(100)));
        assert_eq!(limits.accounts().count(), 2);
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
            claude_account(),
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
            claude_account(),
            RateLimits {
                status: None,
                windows: vec![window(LimitWindow::FiveHour, Some(44.0), None)],
            },
            20,
        );
        let claude = limits.account(&claude_account()).expect("値がある");
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
            claude_account(),
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
        let claude = limits.account(&claude_account()).expect("値がある");
        assert_eq!(
            claude.headline(1_500),
            vec![
                (LimitWindow::Weekly, 18.0),
                (LimitWindow::Named("seven_day_opus".into()), 91.0)
            ]
        );
        assert!(claude.near_limit(1_500));
        assert!(
            limits
                .account(&UsageAccount::local("Codex", None))
                .is_none(),
            "エージェントごとに別"
        );
    }

    /// 止められた状態は、上限に達した窓がリセットされるまで。リセット時刻が変わった窓は使用率も入れ替える。
    #[test]
    fn a_rejection_lasts_until_the_window_resets() {
        let mut limits = UsageLimits::default();
        limits.record(
            claude_account(),
            RateLimits {
                status: Some(LimitStatus::Rejected),
                windows: vec![window(LimitWindow::FiveHour, Some(100.0), Some(1_000))],
            },
            10,
        );
        let claude = limits.account(&claude_account()).expect("値がある");
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
            claude_account(),
            RateLimits {
                status: Some(LimitStatus::Allowed),
                windows: vec![window(LimitWindow::FiveHour, None, Some(19_000))],
            },
            20,
        );
        let claude = limits.account(&claude_account()).expect("値がある");
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
