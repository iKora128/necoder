//! usage — 使用量・レート制限（O11）。スレッドのコストの数え方と、エージェントごとの「最後に受け取った」
//! レート制限の置き場（全ウィンドウの statusbar とポップオーバーが読む）。
//!
//! **値はエージェントが知らせてきた時にしか変わらない。** necoder は利用上限を問い合わせない
//! （資格情報に触れない・常駐のタイマーを持たない）。例外は Codex だけで、ACP に出さない代わりに、
//! 利用者が使用量のポップオーバーを開いた時に `codex app-server` へ 1 回だけ訊く
//! （[`refresh_codex_limits`]・認証は codex 本体）。
//!
//! **報告の無い値を 0 と断定しない**（R08）。トークンやコストを報告しなかったターンは「無い」のまま
//! 台帳へ書き、集計では 0 として足さない（[`reported_total_label`]）。コストはエージェントの推定で、
//! 実際の請求額ではない。

pub use acp_client::usage::{LimitStatus, LimitWindow};
use acp_client::usage::{RateLimits, TurnTokens};
use gpui::{App, SharedString};
use std::collections::BTreeMap;
use std::hash::{BuildHasher as _, RandomState};
use std::sync::OnceLock;

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
/// 認証の環境（設定の `agent_servers.<id>.env`）が違えば別のアカウントかもしれないので、値を混ぜない。
///
/// 認証の環境は 2 つで見分ける: 置き場（`CLAUDE_CONFIG_DIR` / `CODEX_HOME` のパス＝見出しに出してよい）と、
/// それ以外の認証に関わる env（API キー・トークン・base URL 等・[`CREDENTIAL_WORDS`]）の**指紋**。
/// 指紋はハッシュの数値だけで、秘密の値そのものは持たない・出さない・ログに書かない。資格情報の
/// ファイルも読まない（置き場はパスだけ）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UsageKey {
    pub agent: SharedString,
    /// 動かしている場所（手元は空・SSH 先はホストの表示名）。
    pub host: SharedString,
    /// 認証の置き場（無ければ空 = 既定の置き場）。
    pub profile: SharedString,
    /// 置き場以外の認証に関わる env の指紋（[`credential_fingerprint`]）。そういう env が無ければ `None`。
    pub credential_fingerprint: Option<u64>,
    /// セッションに渡した接続（issue #38 H3）。別の会社の契約なので、エージェント自身のログインの値と
    /// 混ぜない。`None` = エージェント自身のログイン。
    pub connection: Option<ConnectionTag>,
}

/// 鍵に入れる接続（名前と、宛先を決める中身の指紋＝ベース URL を書き換えれば別の鍵）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionTag {
    pub name: SharedString,
    pub routing: u64,
}

/// 使用量の鍵の「場所」: 手元は空、SSH 先はホストの表示名（R08）。Host を呼ぶので描画からは使わない。
pub fn host_label(host: &dyn host::Host) -> SharedString {
    if host.is_remote() {
        SharedString::from(host.display_name().to_string())
    } else {
        SharedString::default()
    }
}

/// 認証の置き場を表す env（値はディレクトリのパス＝見出しに出してよい。鍵の `profile` になる）。
const AUTH_PROFILE_ENV: [&str; 2] = ["CLAUDE_CONFIG_DIR", "CODEX_HOME"];

/// 認証に関わる env の名前の語。名前を `_` で区切った語のどれかがこれなら、値を指紋に入れる（R08）。
///
/// **範囲の判断**: env を全部（と起動コマンド）指紋にすると、`MAX_THINKING_TOKENS` や `DEBUG` を
/// 変えただけで同じアカウントが別の行に割れ、見出しにも「設定の env」が付く。ここでは「どのアカウント・
/// どの請求先・どの接続先で動くか」を変え得る語だけを見る。取りこぼすと値が混ざる（R08 の不具合
/// そのもの）ので、迷う語は入れる側に倒す（入れすぎても行が分かれるだけ）。値が空の env は無いのと同じ。
/// - 資格情報: `ANTHROPIC_API_KEY`・`ANTHROPIC_AUTH_TOKEN`・`CLAUDE_CODE_OAUTH_TOKEN`・`OPENAI_API_KEY`・
///   `AWS_SECRET_ACCESS_KEY`・`GH_TOKEN`・`GOOGLE_APPLICATION_CREDENTIALS`・`ANTHROPIC_CUSTOM_HEADERS`
/// - 接続先: `ANTHROPIC_BASE_URL`・`OPENAI_BASE_URL`・`AZURE_OPENAI_ENDPOINT`
/// - アカウントの範囲: `OPENAI_ORG_ID`・`OPENAI_PROJECT`・`GOOGLE_CLOUD_PROJECT`・`AWS_PROFILE`・`AWS_REGION`
/// - 提供元の切り替え: `CLAUDE_CODE_USE_BEDROCK`・`CLAUDE_CODE_USE_VERTEX`・`GOOGLE_GENAI_USE_VERTEXAI`
/// - 資格情報の置き場（パスを見出しに出す [`AUTH_PROFILE_ENV`] 以外）: `XDG_CONFIG_HOME`・`HOME`
///
/// 入れない物: `TOKENS`（`MAX_THINKING_TOKENS` などの上限）・`PROXY`（経路が変わるだけ）・`MODEL`（同じ
/// アカウントの中の選択）。起動コマンドと引数も見ない（env ではない・版を差し替えただけで行が割れる。
/// コマンドの中で資格情報を足すラッパーは、どのみち necoder からは見えない）。
const CREDENTIAL_WORDS: &[&str] = &[
    // 資格情報
    "KEY",
    "APIKEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "CREDENTIALS",
    "AUTH",
    "OAUTH",
    "BEARER",
    "HEADER",
    "HEADERS",
    // 接続先
    "URL",
    "URI",
    "ENDPOINT",
    "HOST",
    // アカウントの範囲
    "ORG",
    "ORGANIZATION",
    "PROJECT",
    "ACCOUNT",
    "PROFILE",
    "TENANT",
    "REGION",
    // 提供元の切り替え
    "BEDROCK",
    "VERTEX",
    "VERTEXAI",
    "AZURE",
    // 資格情報の置き場
    "HOME",
    "CONFIG",
];

/// env の名前が認証に関わるか（[`CREDENTIAL_WORDS`]）。置き場のパスの 2 つは `profile` で持つので除く。
fn is_credential_variable(name: &str) -> bool {
    !AUTH_PROFILE_ENV.contains(&name)
        && name
            .to_ascii_uppercase()
            .split('_')
            .any(|word| CREDENTIAL_WORDS.contains(&word))
}

/// 指紋の塩（プロセスごとの乱数）。画面に出す短い指紋から、秘密の候補を総当たりで確かめられないように
/// する。値は保存しないので、再起動で指紋が変わってよい。
fn fingerprint_salt() -> &'static RandomState {
    static SALT: OnceLock<RandomState> = OnceLock::new();
    SALT.get_or_init(RandomState::new)
}

/// 認証に関わる env（名前と値）の指紋。そういう env が無ければ `None`。値はハッシュに通すだけで、
/// どこにも残さない。
fn credential_fingerprint(env: &BTreeMap<String, String>) -> Option<u64> {
    let credentials: Vec<(&String, &String)> = env
        .iter()
        .filter(|(name, value)| !value.is_empty() && is_credential_variable(name))
        .collect();
    (!credentials.is_empty()).then(|| fingerprint_salt().hash_one(&credentials))
}

impl UsageKey {
    /// 手元・既定の認証の置き場。
    pub fn local(agent: impl Into<SharedString>) -> Self {
        Self {
            agent: agent.into(),
            host: SharedString::default(),
            profile: SharedString::default(),
            credential_fingerprint: None,
            connection: None,
        }
    }

    /// エージェントを動かしている場所（[`host_label`]）と、設定の認証の環境（`agent_servers.<id>.env`）
    /// から作る。Host は呼ばない（描画からも呼ばれる）。
    pub fn for_agent(agent: SharedString, host: SharedString, cx: &App) -> Self {
        // 足したエージェント（H1）も同じ一覧で引く（`start_session` と同じ上書きから鍵を作る）。
        let candidate = settings::agent_by_label(cx, agent.as_ref());
        let agent_override = candidate
            .as_ref()
            .and_then(|candidate| crate::agent_server_override(candidate.id(), cx));
        // 接続は手元のエージェントにだけ渡す（`start_session` と同じ決まり）。
        let connection = candidate
            .filter(|_| host.is_empty())
            .and_then(|candidate| settings::agent_connection_in(cx, candidate.id()));
        Self::with_override(agent, host, agent_override.as_ref())
            .with_connection(connection.as_ref())
    }

    /// 鍵に接続を入れる（`None` = エージェント自身のログイン）。
    pub(crate) fn with_connection(
        mut self,
        connection: Option<&acp_client::connections::Connection>,
    ) -> Self {
        self.connection = connection.map(|connection| ConnectionTag {
            name: SharedString::from(connection.name.clone()),
            routing: connection.routing_fingerprint(),
        });
        self
    }

    /// 設定の上書き（`agent_servers.<id>`）から作る（[`Self::for_agent`] の本体。セッションを立てる時は
    /// 起動に渡すのと同じ上書きからこれで作る）。見るのは env だけ（置き場のパスと、認証に関わる env の指紋）。
    pub(crate) fn with_override(
        agent: SharedString,
        host: SharedString,
        agent_override: Option<&acp_client::AgentOverride>,
    ) -> Self {
        let env = agent_override.map(|agent_override| &agent_override.env);
        let profile = env
            .and_then(|env| {
                AUTH_PROFILE_ENV
                    .iter()
                    .find_map(|key| env.get(*key).cloned())
            })
            .map(SharedString::from)
            .unwrap_or_default();
        Self {
            agent,
            host,
            profile,
            credential_fingerprint: env.and_then(credential_fingerprint),
            connection: None,
        }
    }

    /// 見出し: `Claude Code` / `Claude Code · GLM Coding Plan (Z.ai)`（接続）/ `Claude Code · dev-box` /
    /// `Claude Code · ~/.claude-work` / `Claude Code · 設定の env #1a2b3c`（認証に関わる env の指紋で
    /// 分かれる時。置き場のパスもあればその後ろに添える＝同じ見出しの行を作らない）。指紋は下 24 bit の
    /// 16 進だけを出す。
    pub fn label(&self) -> SharedString {
        let mut label = self.agent.to_string();
        let credentials = self.credential_fingerprint.map(|fingerprint| {
            i18n::t!(
                "usage.credential_env",
                "id" => format!("#{:06x}", fingerprint & 0xff_ffff)
            )
        });
        for part in [
            self.connection
                .as_ref()
                .map(|connection| connection.name.as_ref())
                .unwrap_or_default(),
            self.host.as_ref(),
            self.profile.as_ref(),
            credentials.as_deref().unwrap_or_default(),
        ] {
            if !part.is_empty() {
                label.push_str(" · ");
                label.push_str(part);
            }
        }
        SharedString::from(label)
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

/// エージェントごと（[`UsageKey`] = エージェント + 動かしている場所 + 認証の環境）の、最後に受け取った
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
    /// その読み取りがどの鍵の分か（読みに行った時の手元の Codex × codex へ渡した env）。読み込み中 /
    /// 失敗の行と「再読み込み」はこの鍵の行に出す（別の置き場・別の API キーの行に出さない）。
    pub codex_key: Option<UsageKey>,
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

    /// 手元・既定の認証の環境（置き場も認証に関わる env も足していない）のエージェントの値。
    pub fn agent(&self, agent: &str) -> Option<&AgentLimits> {
        self.get(&UsageKey::from(agent))
    }

    /// 値のある鍵（エージェント名 → 場所 → 置き場の順）。
    pub fn agents(&self) -> impl Iterator<Item = (&UsageKey, &AgentLimits)> {
        self.agents.iter()
    }

    /// Codex を `key`（手元の Codex × codex へ渡す env）で読みに行くかを決め、行くなら読み込み中にする
    /// （[`refresh_codex_limits`] の前半）。同じ鍵なら、読んでいる間と、`force` でなければ直近
    /// [`CODEX_REFRESH_INTERVAL_MS`] 以内は読まない。認証の環境を変えた後（鍵が違う）は、前の鍵の
    /// 読み取りの間隔を待たない（別のアカウントを読む）。
    fn begin_codex_read(&mut self, key: &UsageKey, force: bool, now_ms: i64) -> bool {
        let same_key = self.codex_key.as_ref() == Some(key);
        if same_key
            && (self.codex == CodexRead::Loading
                || (!force && now_ms - self.codex_attempted_at_ms < CODEX_REFRESH_INTERVAL_MS))
        {
            return false;
        }
        self.codex = CodexRead::Loading;
        self.codex_key = Some(key.clone());
        self.codex_attempted_at_ms = now_ms;
        true
    }

    /// Codex の読み取りが終わった（[`refresh_codex_limits`] の後半）。値は読みに行った鍵へ重ねる。
    /// 読んでいる間に認証の環境を変えて別の鍵で読み直していたら、状態（読み込み中 / 失敗）はそちらの
    /// 物なので、この古い読み取りの結果では上書きしない。
    fn finish_codex_read(
        &mut self,
        key: UsageKey,
        result: anyhow::Result<RateLimits>,
        now_ms: i64,
    ) {
        let current = self.codex_key.as_ref() == Some(&key);
        match result {
            Ok(snapshot) => {
                self.record(key, snapshot, now_ms);
                if current {
                    self.codex = CodexRead::Idle;
                }
            }
            Err(error) => {
                eprintln!("Codex の利用上限を読めない: {error:#}");
                if current {
                    self.codex = CodexRead::Failed(SharedString::from(format!("{error:#}")));
                }
            }
        }
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

/// 手元の Codex の鍵（今の設定 `agent_servers.codex` の env で。`codex app-server` を訊く先）。
pub fn local_codex_key(cx: &App) -> UsageKey {
    UsageKey::for_agent(
        SharedString::from(codex_label()),
        SharedString::default(),
        cx,
    )
}

/// Codex の 5 時間枠・週枠を `codex app-server` に訊く（**使用量のポップオーバーを開いた時だけ**・O11）。
///
/// codex が PATH に無ければ何もしない。読んでいる間と、`force` でなければ直近
/// [`CODEX_REFRESH_INTERVAL_MS`] 以内に読みに行った後は重ねない。常駐も定期実行もしない。
/// `settings.json` の `agent_servers.codex.env`（`CODEX_HOME` など）はエージェントと同じものを渡す
/// （同じアカウントを見る）。値は渡した env の鍵（手元の Codex × 置き場・認証に関わる env の指紋）へ
/// 重ねる（R08）。
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
    // 鍵は codex へ渡す env と同じ設定から、読みに行く前に作る（読んでいる間に設定が変わっても、
    // 値は読んだ env の鍵へ重ねる＝別の環境の値と混ぜない）。
    let agent_override = crate::agent_server_override("codex", cx);
    let key = UsageKey::with_override(
        SharedString::from(codex_label()),
        SharedString::default(),
        agent_override.as_ref(),
    );
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(codex.is_some());
    let Some(codex) = codex else {
        return;
    };
    if !limits.begin_codex_read(&key, force, now) {
        return;
    }
    let env = agent_override
        .map(|agent_override| agent_override.env)
        .unwrap_or_default();
    let read = cx
        .background_executor()
        .spawn(async move { acp_client::codex_limits::read_codex_rate_limits(&codex, &env) });
    cx.spawn(async move |cx| {
        let result = read.await;
        cx.update(|cx| {
            cx.default_global::<UsageLimits>()
                .finish_codex_read(key, result, crate::now_unix_ms());
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
    let key = local_codex_key(cx);
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(true);
    limits.codex_key = Some(key.clone());
    limits.record(
        key,
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
    let key = local_codex_key(cx);
    let limits = cx.default_global::<UsageLimits>();
    limits.codex_installed = Some(true);
    limits.codex_key = Some(key);
    limits.codex = read;
}

/// 開発用: 同じ Claude Code の別の鍵（SSH 先・別の置き場・認証に関わる env だけが違う物・置き場 + env）の
/// 値を置く（`NECODER_USAGE_PROBE=keys`・R08 の見出しの検証）。SSH には繋がない。env の値は偽物で、
/// 鍵には指紋しか残らない。
#[cfg(debug_assertions)]
pub fn debug_seed_other_keys(cx: &mut App) {
    use acp_client::usage::WindowUsage;
    let now_ms = crate::now_unix_ms();
    let now_secs = now_ms / 1000;
    let claude = || SharedString::from("Claude Code");
    let with_env = |pairs: &[(&str, &str)]| acp_client::AgentOverride {
        command: None,
        args: Vec::new(),
        env: pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    };
    let keys = [
        (
            UsageKey {
                host: SharedString::from("dev-box"),
                ..UsageKey::local(claude())
            },
            9.0,
            12,
        ),
        (
            UsageKey::with_override(
                claude(),
                SharedString::default(),
                Some(&with_env(&[("CLAUDE_CONFIG_DIR", "~/.claude-work")])),
            ),
            67.0,
            40,
        ),
        (
            UsageKey::with_override(
                claude(),
                SharedString::default(),
                Some(&with_env(&[(
                    "CLAUDE_CODE_OAUTH_TOKEN",
                    "probe-not-a-real-token",
                )])),
            ),
            23.0,
            5,
        ),
        // アカウントの切り替え（O14）が書く長いパス + 接続先: 見出しの中ほどが省かれる幅。
        (
            UsageKey::with_override(
                claude(),
                SharedString::default(),
                Some(&with_env(&[
                    (
                        "CLAUDE_CONFIG_DIR",
                        "/Users/me/Library/Application Support/necoder/accounts/claude/account-2",
                    ),
                    ("ANTHROPIC_BASE_URL", "https://gateway.example.invalid"),
                ])),
            ),
            51.0,
            25,
        ),
    ];
    let limits = cx.default_global::<UsageLimits>();
    for (key, percent, minutes_ago) in keys {
        limits.record(
            key,
            RateLimits {
                status: None,
                windows: vec![WindowUsage {
                    window: LimitWindow::FiveHour,
                    used_percent: Some(percent),
                    resets_at: Some(now_secs + 2 * 3_600),
                }],
            },
            now_ms - minutes_ago * 60_000,
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

    /// 設定 `agent_servers.<id>` の env だけを足した上書き（`type: registry` と同じ形）。
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

    fn claude_key(host: &str, pairs: &[(&str, &str)]) -> UsageKey {
        UsageKey::with_override(
            SharedString::from("Claude Code"),
            SharedString::from(host.to_string()),
            Some(&with_env(pairs)),
        )
    }

    /// R08: 置き場のパスが無くても、認証に関わる env（API キー・トークン・base URL 等）だけが違えば
    /// 別の鍵で、値は混ざらない。同じ env なら同じ鍵（スレッドをまたいで共有する）。認証に関わらない
    /// env は鍵を割らない。
    #[test]
    fn limits_are_kept_apart_per_credential_env() {
        let first = claude_key("", &[("ANTHROPIC_API_KEY", "sk-ant-first")]);
        let second = claude_key("", &[("ANTHROPIC_API_KEY", "sk-ant-second")]);
        assert_ne!(first, second, "API キーの env だけが違えば別の鍵");
        assert!(first.profile.is_empty() && second.profile.is_empty());
        assert_eq!(
            first,
            claude_key("", &[("ANTHROPIC_API_KEY", "sk-ant-first")]),
            "同じ env なら同じ鍵"
        );

        let mut limits = UsageLimits::default();
        let at = |percent: f64| RateLimits {
            status: Some(LimitStatus::Allowed),
            windows: vec![window(LimitWindow::FiveHour, Some(percent), Some(5_000))],
        };
        limits.record(first.clone(), at(12.0), 10);
        limits.record(second.clone(), at(91.0), 20);
        assert_eq!(
            limits.get(&first).map(|limits| limits.headline(100)),
            Some(vec![(LimitWindow::FiveHour, 12.0)])
        );
        assert_eq!(
            limits.get(&second).map(|limits| limits.headline(100)),
            Some(vec![(LimitWindow::FiveHour, 91.0)]),
            "片方の知らせで他方は変わらない"
        );
        assert!(
            limits.agent("Claude Code").is_none(),
            "既定の環境の鍵には何も重ねない"
        );
        assert_eq!(limits.agents().count(), 2);

        // トークン（サブスクのログイン）・接続先も同じ。置き場のパスと env の両方があれば両方で分ける。
        assert_ne!(
            claude_key("", &[("CLAUDE_CODE_OAUTH_TOKEN", "first")]),
            claude_key("", &[("CLAUDE_CODE_OAUTH_TOKEN", "second")])
        );
        assert_ne!(
            claude_key("", &[("ANTHROPIC_BASE_URL", "https://a.example")]),
            claude_key("", &[("ANTHROPIC_BASE_URL", "https://b.example")])
        );
        assert_ne!(
            claude_key(
                "",
                &[("CLAUDE_CONFIG_DIR", "/w"), ("ANTHROPIC_API_KEY", "first")]
            ),
            claude_key(
                "",
                &[("CLAUDE_CONFIG_DIR", "/w"), ("ANTHROPIC_API_KEY", "second")]
            )
        );
        // 認証に関わらない env だけなら既定の環境と同じ鍵（見出しにも何も添えない）。値が空の env も同じ。
        let tuned = claude_key("", &[("MAX_THINKING_TOKENS", "10000"), ("DEBUG", "1")]);
        assert_eq!(tuned, UsageKey::local("Claude Code"));
        assert_eq!(tuned.label().as_ref(), "Claude Code");
        assert_eq!(
            claude_key("", &[("ANTHROPIC_API_KEY", "")]),
            UsageKey::local("Claude Code")
        );
        // 置き場のパスだけなら、指紋は持たず見出しは今までどおり。
        let work = claude_key("", &[("CLAUDE_CONFIG_DIR", "~/.claude-work")]);
        assert_eq!(work.credential_fingerprint, None);
        assert_eq!(work.label().as_ref(), "Claude Code · ~/.claude-work");
    }

    /// R08: 鍵は秘密の値を持たない・出さない（Debug にも見出しにも写らない）。見出しに出すのは短い
    /// 指紋（`#` + 16 進 6 桁）だけ。置き場のパス・SSH 先はそのまま出す。
    #[test]
    fn a_credential_fingerprint_does_not_carry_the_secret() {
        let secret = "sk-ant-api03-do-not-show-this-value";
        let keyed = claude_key(
            "dev-box",
            &[
                ("ANTHROPIC_API_KEY", secret),
                ("CLAUDE_CONFIG_DIR", "/Users/me/.claude-work"),
            ],
        );
        let label = keyed.label();
        let shown = format!("{keyed:?} {label}");
        assert!(!shown.contains(secret), "{shown}");
        assert!(!shown.contains("do-not-show"), "{shown}");
        assert!(
            label.starts_with("Claude Code · dev-box · /Users/me/.claude-work · settings env #"),
            "{label}"
        );
        let id = label.rsplit('#').next().expect("指紋の表示");
        assert!(
            id.len() == 6 && id.chars().all(|character| character.is_ascii_hexdigit()),
            "{label}"
        );

        // 置き場のパスが無く指紋だけで分かれる時は「設定の env #…」だけ。
        let only = UsageKey::with_override(
            SharedString::from("Codex"),
            SharedString::default(),
            Some(&with_env(&[("OPENAI_API_KEY", secret)])),
        );
        assert!(
            only.label().starts_with("Codex · settings env #"),
            "{}",
            only.label()
        );
        assert!(!format!("{only:?} {}", only.label()).contains(secret));
    }

    /// R08: Codex の読み取りは読みに行った鍵へ重ねる。認証の環境を変えた直後に読み直した時、変える前に
    /// 始めた古い読み取りが後から終わっても、新しい鍵の状態（読み込み中 / 失敗 /「再読み込み」の行）を
    /// 上書きしない。
    #[test]
    fn a_stale_codex_read_does_not_overwrite_the_new_key() {
        let codex_key = |pairs: &[(&str, &str)]| {
            UsageKey::with_override(
                SharedString::from("Codex"),
                SharedString::default(),
                Some(&with_env(pairs)),
            )
        };
        let work = codex_key(&[("CODEX_HOME", "/tmp/codex-work")]);
        let keyed = codex_key(&[("OPENAI_API_KEY", "sk-second")]);
        let at = |percent: f64| RateLimits {
            status: None,
            windows: vec![window(LimitWindow::FiveHour, Some(percent), Some(9_000))],
        };
        let percent = |limits: &UsageLimits, key: &UsageKey| {
            limits
                .get(key)
                .and_then(|limits| limits.windows.first())
                .and_then(|reading| reading.used_percent)
        };

        // 古い環境の読み取りの途中で env を変え、新しい環境で読み直す（間隔を待たない）。
        let mut limits = UsageLimits::default();
        assert!(limits.begin_codex_read(&work, false, 1_000));
        assert!(
            !limits.begin_codex_read(&work, false, 2_000),
            "同じ鍵は読んでいる間は重ねない"
        );
        assert!(
            limits.begin_codex_read(&keyed, false, 3_000),
            "env を変えた直後は前の読み取りを待たずに新しい鍵を読む"
        );
        assert_eq!(limits.codex_key.as_ref(), Some(&keyed));

        // 古い読み取りが後から成功: 値は古い鍵へ。状態は新しい鍵の読み込み中のまま。
        limits.finish_codex_read(work.clone(), Ok(at(40.0)), 4_000);
        assert_eq!(limits.codex, CodexRead::Loading);
        assert_eq!(limits.codex_key.as_ref(), Some(&keyed));
        assert_eq!(percent(&limits, &work), Some(40.0));
        assert_eq!(
            percent(&limits, &keyed),
            None,
            "古い値を新しい鍵の値にしない"
        );

        // 新しい読み取りが終われば、値は新しい鍵へ・状態は戻る。
        limits.finish_codex_read(keyed.clone(), Ok(at(10.0)), 5_000);
        assert_eq!(limits.codex, CodexRead::Idle);
        assert_eq!(percent(&limits, &keyed), Some(10.0));
        assert_eq!(percent(&limits, &work), Some(40.0));
        assert!(
            !limits.begin_codex_read(&keyed, false, 6_000),
            "同じ鍵は 60 秒以内に読み直さない"
        );
        assert!(
            limits.begin_codex_read(&keyed, true, 6_000),
            "「再読み込み」は待たない"
        );

        // 古い読み取りが後から失敗しても、新しい鍵の行に失敗を出さない。今の鍵の失敗は出す。
        let mut limits = UsageLimits::default();
        assert!(limits.begin_codex_read(&work, false, 1_000));
        assert!(limits.begin_codex_read(&keyed, false, 2_000));
        limits.finish_codex_read(work.clone(), Err(anyhow::anyhow!("stale failure")), 3_000);
        assert_eq!(limits.codex, CodexRead::Loading);
        assert!(limits.get(&work).is_none(), "失敗は値を残さない");
        limits.finish_codex_read(
            keyed.clone(),
            Err(anyhow::anyhow!("authentication required")),
            4_000,
        );
        assert_eq!(
            limits.codex,
            CodexRead::Failed(SharedString::from("authentication required"))
        );
        assert_eq!(limits.codex_key.as_ref(), Some(&keyed));
    }

    /// 指紋に入れる env の範囲（[`CREDENTIAL_WORDS`]）: 資格情報・接続先・アカウントの範囲・提供元の
    /// 切り替え・資格情報の置き場は入れ、上限や経路の env は入れない。見出しにパスを出す 2 つは
    /// `profile` で持つので除く。
    #[test]
    fn credential_variables_are_told_from_other_env() {
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_CUSTOM_HEADERS",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_ORG_ID",
            "OPENAI_PROJECT",
            "CODEX_API_KEY",
            "AWS_PROFILE",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AWS_REGION",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "ANTHROPIC_VERTEX_PROJECT_ID",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_GENAI_USE_VERTEXAI",
            "GEMINI_API_KEY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "DASHSCOPE_API_KEY",
            "XAI_API_KEY",
            "AZURE_OPENAI_ENDPOINT",
            "XDG_CONFIG_HOME",
            "HOME",
        ] {
            assert!(is_credential_variable(name), "{name} は認証に関わる");
        }
        for name in [
            "MAX_THINKING_TOKENS",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "DEBUG",
            "DISABLE_TELEMETRY",
            "NODE_OPTIONS",
            "HTTPS_PROXY",
            "ANTHROPIC_MODEL",
            "BASH_DEFAULT_TIMEOUT_MS",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
        ] {
            assert!(!is_credential_variable(name), "{name} は指紋に入れない");
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
            "トークンの報告が無いターンは 0 ではなく「無い」（R08）"
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
        // 報告の無い値は 0 と書かない（R08）。
        assert_eq!(reported_total_label(Some("4.0k".into()), 2, 2), "4.0k");
        assert_eq!(
            reported_total_label(Some("4.0k".into()), 1, 2),
            "≥ 4.0k",
            "一部のターンにしか報告が無い合計は下限"
        );
        assert_eq!(
            reported_total_label(None, 0, 2),
            "—",
            "報告が 1 つも無ければ 0 ではなく —"
        );
    }
}
