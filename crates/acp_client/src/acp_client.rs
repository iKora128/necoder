//! acp_client — Claude Code を ACP（Agent Client Protocol）で包む接続層（M4）。
//!
//! ROADMAP M4: `claude-agent-acp` を子プロセスで起動し、ACP で会話する。この層は接続・セッション・
//! プロンプト送信/更新受信までを担い、UI（agent_panel）はチャネル越しに駆動する。
//! まずは **起動 + initialize ハンドシェイク**（＝「繋がる」ことの土台）。session/prompt は継続。
//!
//! 実行時検証: `claude-agent-acp` バイナリ + Claude 認証が要る（実環境で live 検証済み）。

use acp::schema::v1;
use acp::schema::ProtocolVersion;
use agent_client_protocol as acp;
use anyhow::{Context as _, Result};
use futures::channel::mpsc;
use futures::{FutureExt, StreamExt};
use host::{CommandSpec, Host, LocalHost};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

/// 権限リクエストの選択肢の種類（UI のスタイル分け用。ACP `PermissionOptionKind` を簡約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    /// 今回だけ許可。
    Allow,
    /// 常に許可（記憶する）。
    AllowAlways,
    /// 今回だけ拒否。
    Reject,
    /// 常に拒否（記憶する）。
    RejectAlways,
    /// 未知（プロトコル拡張）。中立スタイルで出す。
    Other,
}

/// 権限リクエストの 1 選択肢（許可 / 常に許可 / 拒否 など）。UI がボタンにする。
#[derive(Debug, Clone)]
pub struct PermissionChoice {
    pub label: String,
    pub kind: PermissionKind,
}

/// セッション設定オプションの意味カテゴリ（UI が Model / Effort セレクタへ振り分ける）。
/// ACP `SessionConfigOptionCategory` を簡約（Mode/ModelConfig/Other は今は `Other` に畳む）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigCategory {
    /// モデル選択。
    Model,
    /// 思考/推論レベル（effort 相当）。
    ThoughtLevel,
    /// その他（今は UI で扱わない）。
    Other,
}

/// エージェントが広告する 1 つの選択式設定（モデル・思考レベル等）。
/// ACP `SessionConfigOption` の Select を簡約。UI はこれでセレクタを実選択肢に置き換える。
#[derive(Debug, Clone)]
pub struct ConfigOption {
    pub config_id: String,
    pub category: ConfigCategory,
    /// 現在の value_id。
    pub current: String,
    /// 選択肢 `(value_id, 表示名)`。
    pub choices: Vec<(String, String)>,
}

/// 権限リクエストに含まれるファイル編集の差分（accept/reject の diff レビュー用）。
#[derive(Debug, Clone)]
pub struct PermissionDiff {
    pub path: String,
    /// 変更前の内容（新規ファイルなら `None`）。
    pub old_text: Option<String>,
    pub new_text: String,
}

/// ツール呼び出しの種別（ACP `ToolKind` の UI 非依存な写し）。表示の分岐に使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

/// 1 ツール呼び出しの情報（`ToolCall` = 開始 / `ToolCallUpdate` = 更新 を UI 非依存に簡約）。
/// transcript に「何をした・どのファイル・before/after・出力」を出すための素材。
/// 更新では未変更フィールドは None / 空（＝据え置き）で届く。
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    /// 相関 ID（開始と更新を結ぶ）。
    pub id: String,
    /// 人間可読タイトル（例「Edit src/main.rs」）。更新では None のことがある。
    pub title: Option<String>,
    pub kind: Option<ToolCallKind>,
    /// 触ったファイル（パス）。
    pub locations: Vec<String>,
    /// ファイル編集の差分（Edit 系。before/after）。
    pub diffs: Vec<PermissionDiff>,
    /// 実行出力など本文テキスト（Bash 出力・要約等）。
    pub output: Option<String>,
    /// 完了したか（Some(true)=成功 / Some(false)=失敗 / None=進行中・不明）。
    pub completed: Option<bool>,
}

/// プラン 1 項目の状態（ACP `PlanEntryStatus` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStatus {
    /// 未着手。
    Pending,
    /// 進行中（UI では ● スレッド色）。
    InProgress,
    /// 完了。
    Completed,
}

/// エージェントの実行プラン 1 項目（ACP `SessionUpdate::Plan` の写し・M12-9）。
/// プランは毎回**全量置換**で届く（差分ではない）。
#[derive(Debug, Clone)]
pub struct PlanItem {
    pub content: String,
    pub status: PlanStatus,
}

/// UI へ流すストリーミングイベント（ACP の `SessionUpdate` を UI 非依存に簡約したもの）。
/// agent_panel はこれを受けて transcript を逐次更新する。
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// エージェント本文の増分テキスト（`AgentMessageChunk`）。
    AgentChunk(String),
    /// 思考の増分テキスト（`AgentThoughtChunk`）。
    ThoughtChunk(String),
    /// ツール呼び出しの開始（`ToolCall`）。タイトル・種別・触ったファイル・差分・出力を含む。
    ToolStarted(ToolCallInfo),
    /// ツール呼び出しの更新（`ToolCallUpdate`）。実行後の出力・差分・完了状態が後追いで届く。
    /// `id` で開始時のエントリに紐づける。
    ToolUpdated(ToolCallInfo),
    /// コンテキスト使用量の更新（`UsageUpdate`）。`used`/`size` はトークン数。
    Usage { used: u64, size: u64 },
    /// エージェントが広告する権限モード一覧 + 現在モード（セッション開始時）。`(mode_id, 表示名)`。
    Modes {
        modes: Vec<(String, String)>,
        current: String,
    },
    /// 現在モードが変わった（`CurrentModeUpdate`）。mode_id。
    ModeChanged(String),
    /// エージェントが広告する設定オプション（モデル・思考レベル等）。セッション開始時 + 変更時。
    /// 空でも「広告あり（＝実反映できる）」の意味で送る。UI は該当セレクタを実選択肢に置き換える。
    Configs(Vec<ConfigOption>),
    /// エージェントがツール実行/ファイル編集の許可を求めてきた（`session/request_permission`）。
    /// `respond` に**選んだ選択肢の添字**を送ると応答する。sender を drop するとキャンセル扱い。
    /// このイベントの間、当該ターンはエージェント側でブロックしている（応答するまで進まない）。
    PermissionRequest {
        title: String,
        diffs: Vec<PermissionDiff>,
        options: Vec<PermissionChoice>,
        respond: mpsc::UnboundedSender<usize>,
    },
    /// エージェントの実行プラン全量（`SessionUpdate::Plan`）。UI は常設チェックリストへ置換反映する。
    Plan(Vec<PlanItem>),
    /// 1 ターン（prompt→応答）が完了した（`StopReason`）。`reason` で正常完了/中断を区別する。
    TurnEnded { reason: TurnEnd },
    /// エラー（接続断・プロトコル異常・起動失敗など）。
    Failed(String),
}

/// ターンの終わり方（ACP `StopReason` の簡約）。UI の「完了/中断」の出し分けに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// 正常完了 or 上限到達（`EndTurn` / `MaxTokens` / `MaxTurnRequests`）。
    Completed,
    /// 中断（`Refusal` / `Cancelled`）＝ユーザーの注意を要する終わり方。完了音は鳴らさない。
    Interrupted,
}

/// UI → 常駐セッションへの指示（[`run_session`] が単一チャネルで受ける）。
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// prompt を送る。
    Prompt(String),
    /// 実行中のターンを中断する（`session/cancel` 通知）。エージェントは
    /// `StopReason::Cancelled` でターンを畳むので、UI には `TurnEnded { Interrupted }` が届く。
    /// **ターン中に受け取れる**必要があるため、ターンループが `read_update` と同時に待つ。
    Cancel,
    /// 権限モードを変更する（`session/set_mode`。引数は mode_id）。
    SetMode(String),
    /// 設定オプション（モデル・思考レベル等）を変更する（`session/set_config_option`）。
    SetConfig { config_id: String, value_id: String },
}

/// ターンループが待つ 2 系統（エージェントからの更新 / UI からのコマンド）。
/// `select!` の戻り値を所有型にして、`session` と `command_rx` の借用をブロック内で閉じる。
enum TurnEvent {
    Update(Result<acp::SessionMessage, acp::Error>),
    Command(Option<SessionCommand>),
}

/// ACP エージェント（claude-agent-acp）の起動設定。
pub struct AgentCommand {
    pub path: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl AgentCommand {
    /// 既定エージェント（Claude）の起動コマンド。`cwd` はプロジェクトルート。互換用。
    pub fn claude(cwd: impl Into<PathBuf>) -> Option<AgentCommand> {
        AGENTS.first()?.command(cwd)
    }
}

/// 選べる ACP エージェント（Zed の external_agents レジストリ準拠）。
/// `bin` は Zed の npx キャッシュ `.bin/` 名、`package` は npx で落とすパッケージ、`extra_args` は
/// ACP モードに入るための追加引数（`-acp` パッケージは不要、gemini/copilot/qwen は `--acp` 等）。
pub struct AgentKind {
    pub id: &'static str,
    pub label: &'static str,
    /// vendor CLI 本体。ACP adapter の `bin` とは別（Claude/Codex は特に別 package）。
    cli_bin: &'static str,
    bin: &'static str,
    /// npx フォールバック用の npm パッケージ。**npm 外（kimi=PyPI 等）は None**。
    package: Option<&'static str>,
    extra_args: &'static [&'static str],
    /// セットアップ画面の「入れ方」でターミナルに流す導入コマンド（vendor の CLI 本体を入れる）。
    pub install_cmd: &'static str,
    /// セットアップ画面の「ログイン」でターミナルに流す認証コマンド（vendor 自身のログイン導線）。
    /// Shirushi は鍵を持たず、CLI 側の認証にそのまま乗る（Zed の ACP と同じ流儀）。
    pub login_cmd: &'static str,
    /// ブランドアイコンの svg パス（設定画面・スレッドタブで共用）。在庫が無いものは `None`＝モノグラム表示。
    /// カタログ＝アイコンの単一の出所（settings/agent_panel が共に acp_client を依存に持つため）。
    pub icon: Option<&'static str>,
    /// ブランド色（アイコン tint・モノグラム背景）。0xRRGGBB。
    pub brand_color: u32,
    /// アイコンが無い時に出す短いモノグラム（例 codex=">_"）。
    pub monogram: &'static str,
}

/// エージェントのローカル導入状況（設定画面のステータス表示）。認証状態は見ない（CLI 任せ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// bin がローカルにある（PATH / Zed の npx キャッシュ）。すぐ使える。
    Installed,
    /// bin は無いが npm パッケージがあり npx で初回取得できる。
    Npx,
    /// bin も無く npm 外＝手動導入が要る（例: Kimi=uv/pip）。
    Missing,
}

/// 対応エージェント一覧。先頭（Claude）が既定。Claude 以外は初回 npx/導入 + 各サービス認証が要る。
/// `package` は npx フォールバック用（npm 外＝Kimi は None＝PATH の bin を使う）。`install_cmd`/
/// `login_cmd` はセットアップ画面がターミナルに流す人間向けコマンド（vendor 自身の導線に委譲）。
pub const AGENTS: &[AgentKind] = &[
    AgentKind {
        id: "claude",
        label: "Claude Code",
        cli_bin: "claude",
        bin: "claude-agent-acp",
        package: Some("@agentclientprotocol/claude-agent-acp@0.66.0"),
        extra_args: &[],
        install_cmd: "npm i -g @anthropic-ai/claude-code",
        login_cmd: "claude auth login",
        icon: Some("icons/brand-claude.svg"),
        brand_color: 0xd9_77_57,
        monogram: "C",
    },
    AgentKind {
        id: "codex",
        label: "Codex",
        cli_bin: "codex",
        bin: "codex-acp",
        package: Some("@agentclientprotocol/codex-acp@1.1.14"),
        extra_args: &[],
        install_cmd: "npm i -g @openai/codex",
        login_cmd: "codex login",
        icon: None,
        brand_color: 0x10_a3_7f,
        monogram: ">_",
    },
    // 旧 Gemini CLI は 2026-06-18 に廃止 → 後継 Antigravity CLI（`agy`）はクローズド Go 書き直しで **ACP 非対応**
    // （`--acp`/acp サブコマンド無し・実機 + ドキュメント確認済み）。よって会話エージェント一覧から除外。
    // Antigravity が ACP を出したら再追加する（`agy -p` の title 生成自体は動くが、会話に選べない＝utility も使われない）。
    AgentKind {
        id: "copilot",
        label: "GitHub Copilot",
        cli_bin: "copilot",
        bin: "copilot",
        package: Some("@github/copilot@1.0.70"),
        extra_args: &["--acp"],
        install_cmd: "npm i -g @github/copilot",
        login_cmd: "copilot login",
        icon: Some("icons/brand-copilot.svg"),
        brand_color: 0xd0_d5_db,
        monogram: "Co",
    },
    AgentKind {
        id: "qwen",
        label: "Qwen Code",
        cli_bin: "qwen",
        bin: "qwen",
        package: Some("@qwen-code/qwen-code@0.19.9"),
        extra_args: &["--acp", "--experimental-skills"],
        install_cmd: "npm i -g @qwen-code/qwen-code",
        login_cmd: "qwen",
        icon: Some("icons/brand-qwen.svg"),
        brand_color: 0x69_50_ef,
        monogram: "Q",
    },
    // OpenCode（sst）: ACP は `opencode acp` サブコマンド。npm `opencode-ai`。
    AgentKind {
        id: "opencode",
        label: "OpenCode",
        cli_bin: "opencode",
        bin: "opencode",
        package: Some("opencode-ai"),
        extra_args: &["acp"],
        install_cmd: "npm i -g opencode-ai",
        login_cmd: "opencode auth login",
        icon: Some("icons/brand-opencode.svg"),
        brand_color: 0xd0_d5_db,
        monogram: "OC",
    },
    // Kimi Code CLI（Moonshot）: ACP は `kimi acp`。PyPI `kimi-cli`（npm 外＝package None）。
    AgentKind {
        id: "kimi",
        label: "Kimi CLI",
        cli_bin: "kimi",
        bin: "kimi",
        package: None,
        extra_args: &["acp"],
        install_cmd: "uv tool install kimi-cli",
        login_cmd: "kimi login",
        icon: Some("icons/brand-kimi.svg"),
        brand_color: 0xd0_d5_db,
        monogram: "K",
    },
    // Grok Build（xAI・Rust TUI）: curl 導入・初回はブラウザ認証。ACP は `grok acp`
    // （opencode/kimi と同じ Rust 系の慣例。README 未記載のため要実機確認）。
    AgentKind {
        id: "grok",
        label: "Grok Build",
        cli_bin: "grok",
        bin: "grok",
        package: None,
        extra_args: &["acp"],
        install_cmd: "curl -fsSL https://x.ai/cli/install.sh | bash",
        login_cmd: "grok login",
        icon: None,
        brand_color: 0x4b_55_63,
        monogram: "G",
    },
];

/// UI のエージェントセレクタに出すラベル一覧（[`AGENTS`] と 1:1 対応・Zed の registry 表示名準拠）。
pub const AGENT_LABELS: &[&str] = &[
    "Claude Code",
    "Codex",
    "GitHub Copilot",
    "Qwen Code",
    "OpenCode",
    "Kimi CLI",
    "Grok Build",
];

/// 設定画面が表示する Agent の利用状態。資格情報そのものは読み出さず、CLI の status、
/// 設定ファイルの存在、またはプロンプト無しの ACP session probe だけで判定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAuthState {
    /// 認証済み status、または ACP initialize → session/new に成功。選択可能。
    Available,
    /// 資格情報・provider 設定は見つかったが、オンラインでの有効性は未確認。
    Configured,
    /// CLI は未導入、または確認できる認証情報が無い。
    SignedOut,
}

static AGENT_AUTH_STATES: OnceLock<RwLock<Vec<AgentAuthState>>> = OnceLock::new();

/// 利用可能、または資格情報の存在まで確認できた Agent。composer はこの一覧を選択肢へ出す。
/// `Configured` は Settings の背景 refresh で session 成功後に `Available` へ昇格する。
pub fn authenticated_agent_labels() -> Vec<&'static str> {
    let states = cached_agent_auth_states();
    AGENTS
        .iter()
        .zip(states)
        .filter_map(|(agent, state)| (state != AgentAuthState::SignedOut).then_some(agent.label))
        .collect()
}

/// 最後に確認した全 Agent の状態。初回はローカルの軽い CLI/config 判定だけを行う。
pub fn cached_agent_auth_states() -> Vec<AgentAuthState> {
    AGENT_AUTH_STATES
        .get_or_init(|| RwLock::new(detect_configured_agent_states()))
        .read()
        .map(|states| states.clone())
        .unwrap_or_else(|_| vec![AgentAuthState::SignedOut; AGENTS.len()])
}

/// CLI/config 判定に加え、必要な Agent はプロンプト無しの ACP session を短時間だけ開く。
/// probe は並列・タイムアウト付きで、終了時に子プロセスを必ず kill/wait する。
pub async fn refresh_agent_auth_states(cwd: impl Into<PathBuf>) -> Vec<AgentAuthState> {
    let cwd = cwd.into();
    let initial = detect_configured_agent_states();
    let probes = AGENTS
        .iter()
        .zip(initial.iter().copied())
        .map(|(agent, state)| {
            let cwd = cwd.clone();
            async move {
                if !agent.cli_installed() {
                    return state;
                }
                // status コマンドは最大 2 秒掛かり得るので、この明示 refresh の背景処理にだけ置く。
                // cached_agent_auth_states()/composer render からは絶対に呼ばない（起動遅延の根治）。
                let status_available = match agent.id {
                    "claude" => command_succeeds("claude", &["auth", "status"]),
                    "codex" => command_succeeds("codex", &["login", "status"]),
                    "copilot" => command_succeeds("gh", &["auth", "status"]),
                    _ => false,
                };
                if status_available || state == AgentAuthState::Available {
                    return AgentAuthState::Available;
                }
                if agent.probe_acp_session(cwd).await {
                    AgentAuthState::Available
                } else {
                    state
                }
            }
        });
    let states = futures::future::join_all(probes).await;
    let shared = AGENT_AUTH_STATES.get_or_init(|| RwLock::new(Vec::new()));
    if let Ok(mut current) = shared.write() {
        *current = states.clone();
    }
    states
}

fn detect_configured_agent_states() -> Vec<AgentAuthState> {
    AGENTS
        .iter()
        .map(AgentKind::configured_auth_state)
        .collect()
}

impl AgentKind {
    /// ラベル（例 "Claude"）から引く。
    pub fn by_label(label: &str) -> Option<&'static AgentKind> {
        AGENTS.iter().find(|agent| agent.label == label)
    }

    /// ブランド表示 `(svg パス, モノグラム, ブランド色 0xRRGGBB)`。設定画面・タブで共用。
    pub fn brand(&self) -> (Option<&'static str>, &'static str, u32) {
        (self.icon, self.monogram, self.brand_color)
    }

    /// utility（スレッドタイトル等の一発生成）に使う、既定 Agent ごとの **shell テンプレート**。
    /// プレースホルダ: `{prompt}`=指示・`{excerpt}`=会話冒頭ファイル・`{out}`=最終メッセージ出力先。
    /// stdout にクリーンなタイトルが載るよう各 CLI 差を吸収する（claude -p は素で stdout・codex exec は
    /// agent 実行で stdout が汚いため `--output-last-message`+`cat` で拾う）。`--model` は付けない
    /// ＝各 CLI の既定モデル（＝ユーザーが使い込む既定）をそのまま使う。
    /// **claude/codex は実機検証済み**。未対応（None）は utility スキップ＝
    /// タイトルは既定名のまま（壊れない・Claude 決め打ちもしない）。
    pub fn oneshot(&self) -> Option<&'static str> {
        match self.id {
            "claude" => Some(r#"claude -p "{prompt}" < {excerpt}"#),
            "codex" => Some(
                r#"codex exec --sandbox read-only --color never --output-last-message {out} "{prompt}" < {excerpt} >/dev/null 2>&1 && cat {out}"#,
            ),
            _ => None,
        }
    }

    /// このエージェントの起動コマンドを解決する。
    /// 探索順: (1) PATH の単体バイナリ → (2) Zed の npx キャッシュ(.bin) → (3) `npx <package> <args>`。
    pub fn command(&self, cwd: impl Into<PathBuf>) -> Option<AgentCommand> {
        let cwd = cwd.into();
        let extra: Vec<String> = self.extra_args.iter().map(|arg| arg.to_string()).collect();
        // 1) PATH の単体バイナリ
        if let Some(path) = find_in_path(self.bin) {
            return Some(AgentCommand {
                path,
                args: extra,
                cwd,
            });
        }
        // 2) Zed が展開済みの npx キャッシュ（ネット不要）
        if let Some(bin) = zed_cached_agent(self.bin) {
            return Some(AgentCommand {
                path: bin,
                args: extra,
                cwd,
            });
        }
        // 3) npx フォールバック（npm パッケージがある agent のみ。node/npx が PATH に要る）
        let package = self.package?;
        let npx = find_in_path("npx")?;
        let mut args = vec!["-y".to_string(), package.to_string()];
        args.extend(extra);
        Some(AgentCommand {
            path: npx,
            args,
            cwd,
        })
    }

    /// ローカルでの導入状況（設定画面のステータス表示用）。認証状態までは見ない（＝CLI 任せ）。
    pub fn availability(&self) -> Availability {
        if find_in_path(self.bin).is_some() || zed_cached_agent(self.bin).is_some() {
            Availability::Installed
        } else if self.package.is_some() && find_in_path("npx").is_some() {
            Availability::Npx // bin は無いが npx で初回取得できる
        } else {
            Availability::Missing // bin も無く npm 外（要手動導入。例: Kimi=uv）
        }
    }

    /// vendor CLI 本体が PATH にあるか。ACP adapter の導入状態とは混同しない。
    pub fn cli_installed(&self) -> bool {
        find_in_path(self.cli_bin).is_some()
    }

    /// 対話も子プロセス起動もしない軽量判定。composer の初回 render から呼ばれるため、
    /// ファイル存在と環境変数だけを見る。status/probe は明示的な背景 refresh に分離する。
    fn configured_auth_state(&self) -> AgentAuthState {
        if !self.cli_installed() {
            return AgentAuthState::SignedOut;
        }
        let available = match self.id {
            "claude" => env_has_any(&["ANTHROPIC_API_KEY"]),
            "codex" => env_has_any(&["OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"]),
            _ => false,
        };
        if available {
            return AgentAuthState::Available;
        }

        let configured = match self.id {
            "claude" => home_path_exists(".claude/.credentials.json"),
            "codex" => home_path_exists(".codex/auth.json"),
            "copilot" => {
                env_has_any(&[
                    "COPILOT_GITHUB_TOKEN",
                    "GH_TOKEN",
                    "GITHUB_TOKEN",
                    "COPILOT_PROVIDER_API_KEY",
                ]) || home_path_exists(".copilot/config.json")
                    || home_path_exists(".config/gh/hosts.yml")
            }
            "qwen" => {
                env_has_any(&[
                    "DASHSCOPE_API_KEY",
                    "OPENAI_API_KEY",
                    "ANTHROPIC_API_KEY",
                    "GEMINI_API_KEY",
                ]) || home_path_exists(".qwen/settings.json")
                    || home_path_exists(".qwen/.env")
            }
            "opencode" => {
                env_has_any(&["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"])
                    || opencode_auth_exists()
            }
            "kimi" => {
                env_has_any(&["KIMI_API_KEY"])
                    || home_path_exists(".kimi-code/config.toml")
                    || home_path_exists(".kimi-code/credentials.json")
            }
            "grok" => {
                env_has_any(&["XAI_API_KEY"])
                    || home_path_exists(".grok/config.toml")
                    || home_path_exists(".grok/credentials.json")
            }
            _ => false,
        };
        if configured {
            AgentAuthState::Configured
        } else {
            AgentAuthState::SignedOut
        }
    }

    /// ネット取得を発生させず、既にある ACP adapter だけを解決する。
    fn installed_command(&self, cwd: impl Into<PathBuf>) -> Option<AgentCommand> {
        let cwd = cwd.into();
        let path = find_in_path(self.bin).or_else(|| zed_cached_agent(self.bin))?;
        Some(AgentCommand {
            path,
            args: self
                .extra_args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect(),
            cwd,
        })
    }

    /// initialize → session/new までを短時間だけ実行する。prompt は送らないため利用料は発生せず、
    /// タイムアウト・完了のどちらでも [`host::HostProcess`] の Drop が子を kill/wait する。
    async fn probe_acp_session(&self, cwd: impl Into<PathBuf>) -> bool {
        let Some(command) = self.installed_command(cwd) else {
            return false;
        };
        probe_session(&command, Duration::from_secs(4)).await
    }

    /// 指定 host 上で agent を解決する。remote の認証情報は remote 側のものだけを使う。
    pub fn command_on(&self, host: &dyn Host, cwd: impl Into<PathBuf>) -> Option<AgentCommand> {
        let cwd = cwd.into();
        if !host.is_remote() {
            return self.command(cwd);
        }
        let resolve = |binary: &str| {
            let output = host
                .run_command(&CommandSpec::new("sh", &cwd).args([
                    "-lc".to_string(),
                    format!("command -v -- {}", shell_word(binary)),
                ]))
                .ok()?;
            output
                .success()
                .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string()))
        };
        let extra: Vec<String> = self.extra_args.iter().map(|arg| arg.to_string()).collect();
        if let Some(path) = resolve(self.bin) {
            return Some(AgentCommand {
                path,
                args: extra,
                cwd,
            });
        }
        let package = self.package?;
        let npx = resolve("npx")?;
        let mut args = vec!["-y".to_string(), package.to_string()];
        args.extend(extra);
        Some(AgentCommand {
            path: npx,
            args,
            cwd,
        })
    }
}

async fn probe_session(command: &AgentCommand, timeout: Duration) -> bool {
    let spec =
        CommandSpec::new(command.path.to_string_lossy(), &command.cwd).args(command.args.clone());
    let mut process = match LocalHost::shared().spawn_process(&spec) {
        Ok(process) => process,
        Err(_) => return false,
    };
    let stdin = match process.take_stdin() {
        Ok(stdin) => stdin,
        Err(_) => return false,
    };
    let stdout = match process.take_stdout() {
        Ok(stdout) => stdout,
        Err(_) => return false,
    };
    let transport = acp::ByteStreams::new(
        blocking::Unblock::new(stdin),
        blocking::Unblock::new(stdout),
    );
    let cwd = command.cwd.clone();
    let probe = FutureExt::fuse(acp::Client.builder().connect_with(
        transport,
        async move |connection| {
            connection
                .send_request(initialize_request())
                .block_task()
                .await?;
            connection
                .send_request(v1::NewSessionRequest::new(&cwd))
                .block_task()
                .await?;
            Ok::<(), acp::Error>(())
        },
    ));
    let deadline = FutureExt::fuse(async_io::Timer::after(timeout));
    futures::pin_mut!(probe, deadline);
    futures::select! {
        result = probe => result.is_ok(),
        _ = deadline => false,
    }
}

fn env_has_any(names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn home_path_exists(relative: impl AsRef<Path>) -> bool {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .is_some_and(|home| home.join(relative).exists())
}

fn opencode_auth_exists() -> bool {
    let path = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|root| root.join("opencode/auth.json"));
    path.and_then(|path| std::fs::metadata(path).ok())
        .is_some_and(|metadata| metadata.is_file() && metadata.len() > 2)
}

fn command_succeeds(binary: &str, args: &[&str]) -> bool {
    let Some(path) = find_in_path(binary) else {
        return false;
    };
    let mut child = match Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) | Err(_) => {
                let _kill = child.kill();
                let _wait = child.wait();
                return false;
            }
        }
    }
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Zed の npx キャッシュから指定 `.bin/<name>` を探す（macOS）。node シェバングなので node が PATH に要る。
/// `~/Library/Application Support/Zed/node/cache/_npx/<hash>/node_modules/.bin/<name>`。
fn zed_cached_agent(bin: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let cache = PathBuf::from(home).join("Library/Application Support/Zed/node/cache/_npx");
    for entry in std::fs::read_dir(&cache).ok()?.flatten() {
        let candidate = entry.path().join("node_modules/.bin").join(bin);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// PATH からバイナリを探す。
pub fn find_in_path(binary: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

/// 我々（クライアント）の能力を広告する initialize リクエスト。
/// **設定オプション（モデル・思考レベル）** を受け取るには config_options 能力の広告が要る
/// （広告しないとエージェントが `config_options` を送ってこないことがある）。
fn initialize_request() -> v1::InitializeRequest {
    v1::InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
        v1::ClientCapabilities::new().session(
            v1::ClientSessionCapabilities::new().config_options(
                v1::SessionConfigOptionsCapabilities::new()
                    .boolean(v1::BooleanConfigOptionCapabilities::new()),
            ),
        ),
    )
}

/// ACP の `SessionConfigOption` 群を UI 非依存の [`ConfigOption`] へ簡約する（Select のみ扱う）。
fn map_config_options(options: &[v1::SessionConfigOption]) -> Vec<ConfigOption> {
    options.iter().filter_map(map_config_option).collect()
}

fn map_config_option(option: &v1::SessionConfigOption) -> Option<ConfigOption> {
    let category = match &option.category {
        Some(v1::SessionConfigOptionCategory::Model) => ConfigCategory::Model,
        Some(v1::SessionConfigOptionCategory::ThoughtLevel) => ConfigCategory::ThoughtLevel,
        _ => ConfigCategory::Other,
    };
    match &option.kind {
        v1::SessionConfigKind::Select(select) => {
            let choices = match &select.options {
                v1::SessionConfigSelectOptions::Ungrouped(options) => options
                    .iter()
                    .map(|option| (option.value.to_string(), option.name.clone()))
                    .collect(),
                v1::SessionConfigSelectOptions::Grouped(groups) => groups
                    .iter()
                    .flat_map(|group| {
                        group
                            .options
                            .iter()
                            .map(|option| (option.value.to_string(), option.name.clone()))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            Some(ConfigOption {
                config_id: option.id.to_string(),
                category,
                current: select.current_value.to_string(),
                choices,
            })
        }
        _ => None, // Boolean は今は UI で扱わない
    }
}

/// ACP ハンドシェイク（initialize）応答の上限。これを超えたら「エージェントが無言でハング」とみなし、
/// 無限に待ち続けず**エラーを返す**（＝チャットに「エラー: …」として出て、無言のスピナー継続が断てる）。
/// ローカル起動の initialize は通常 1〜3 秒。初回だけ npx が shim を取得する分の余裕を見て、host の
/// `REQUEST_TIMEOUT` と同じ 30 秒。プロセスが即死した場合は stdout の EOF で即エラーになる（この
/// timeout は**プロセスが生きたまま応答しない“真の無言ハング”専用**の最後の砦）。
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// ACP リクエスト `future` を [`HANDSHAKE_TIMEOUT`] 付きで待つ薄いラッパ（各ハンドシェイクの入口）。
async fn with_handshake_timeout<T>(
    label: &str,
    future: impl std::future::Future<Output = std::result::Result<T, acp::Error>>,
) -> std::result::Result<T, acp::Error> {
    with_timeout(HANDSHAKE_TIMEOUT, label, future).await
}

/// `future` を `timeout` 付きで待つ。時間切れなら無言ハングを表す `acp::Error`（closure の戻り型＝
/// `Result<_, acp::Error>` に合わせるので、呼び出し側の `?` / `.context()` はそのまま効く）。理由は
/// `message` に直接入れる＝UI（`AgentEvent::Failed`）へ「エラー: … が N 秒応答しません」と素直に出る。
/// タイマは stdio と同じ `blocking` プールで寝るスレッド＝新規依存なし・ACP の実行ランタイム非依存。
/// `timeout` を引数にするのはテストから短い値を渡し、30 秒待たずに挙動を検証できるようにするため。
async fn with_timeout<T>(
    timeout: std::time::Duration,
    label: &str,
    future: impl std::future::Future<Output = std::result::Result<T, acp::Error>>,
) -> std::result::Result<T, acp::Error> {
    use futures::future::{Either, select};
    let timer = blocking::unblock(move || std::thread::sleep(timeout));
    futures::pin_mut!(future, timer);
    match select(future, timer).await {
        Either::Left((result, _timer)) => result,
        Either::Right(((), _future)) => Err(acp::Error::new(
            i32::from(acp::ErrorCode::InternalError),
            format!("{label} が {} 秒応答しません（エージェントの無言ハング）", timeout.as_secs()),
        )),
    }
}

/// エージェントを起動し、ACP の initialize ハンドシェイクまで行う。
/// 返り値は初期化応答（プロトコル版・エージェント能力）。session/prompt はこの接続に積んでいく（M4 継続）。
pub async fn connect_and_initialize(command: &AgentCommand) -> Result<v1::InitializeResponse> {
    let mut child = Command::new(&command.path)
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit()) // エージェントのログは我々の stderr へ（検証しやすく）
        .spawn()
        .with_context(|| {
            format!(
                "claude-agent-acp を起動できない: {}",
                command.path.display()
            )
        })?;

    let stdin = child.stdin.take().context("子プロセスの stdin が無い")?;
    let stdout = child.stdout.take().context("子プロセスの stdout が無い")?;
    // 同期パイプを futures の AsyncWrite / AsyncRead へ（ACP crate 自身の stdio と同じ blocking::Unblock 手法）
    let transport = acp::ByteStreams::new(
        blocking::Unblock::new(stdin),
        blocking::Unblock::new(stdout),
    );

    let response = acp::Client
        .builder()
        .connect_with(transport, async |connection| {
            with_handshake_timeout(
                "ACP initialize",
                connection.send_request(initialize_request()).block_task(),
            )
            .await
        })
        .await
        .context("ACP initialize に失敗")?;

    Ok(response)
}

/// エージェントを起動し、initialize → 新規セッション → 1 プロンプト送信 → 応答テキストを集約して返す。
/// 1 回で完結する非ストリーミング版（毎回プロセスを起動する簡易実装）。パネルはこれを GPUI の
/// バックグラウンドタスクで呼び、結果を transcript に流す。ストリーミング（`read_update` の逐次反映）は継続。
pub async fn prompt_once(command: &AgentCommand, prompt: &str) -> Result<String> {
    let mut child = Command::new(&command.path)
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| {
            format!(
                "claude-agent-acp を起動できない: {}",
                command.path.display()
            )
        })?;

    let stdin = child.stdin.take().context("子プロセスの stdin が無い")?;
    let stdout = child.stdout.take().context("子プロセスの stdout が無い")?;
    let transport = acp::ByteStreams::new(
        blocking::Unblock::new(stdin),
        blocking::Unblock::new(stdout),
    );

    // クロージャは 'static になり得るよう所有データを move する（借用を持ち込まない）。
    let prompt = prompt.to_string();
    let cwd = command.cwd.clone();
    let result = acp::Client
        .builder()
        .connect_with(transport, async move |connection| {
            with_handshake_timeout(
                "ACP initialize",
                connection.send_request(initialize_request()).block_task(),
            )
            .await?;
            let mut session = connection.build_session(&cwd).block_task().start_session().await?;
            session.send_prompt(prompt)?;
            session.read_to_string().await
        })
        .await
        .context("ACP prompt に失敗");

    // 後始末: 子プロセスを終了して回収する。既に終了済みなら kill は失敗する（想定内）。
    let _killed = child.kill();
    if let Err(error) = child.wait() {
        eprintln!("claude-agent-acp の回収に失敗: {error}");
    }
    result
}

/// **常駐セッション + 逐次ストリーミング**。エージェントを起動して 1 セッションを開き、`prompt_rx` から
/// 届く各 prompt を送っては `session/update` を [`AgentEvent`] に簡約して `event_tx` へ逐次流す。
/// `prompt_rx` が閉じる（＝送信ハンドルが全て drop）まで常駐し、プロセス・セッションを保持する
/// （同一スレッド内は文脈が続く）。ターン境界は `StopReason` = [`AgentEvent::TurnEnded`]。
pub async fn run_session(
    command: AgentCommand,
    command_rx: mpsc::UnboundedReceiver<SessionCommand>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
) -> Result<()> {
    run_session_on(LocalHost::shared(), command, command_rx, event_tx).await
}

/// 指定 host 上の常駐 ACP セッション。remote filesystem と agent process を同居させる。
pub async fn run_session_on(
    host: Arc<dyn Host>,
    command: AgentCommand,
    mut command_rx: mpsc::UnboundedReceiver<SessionCommand>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
) -> Result<()> {
    let spec =
        CommandSpec::new(command.path.to_string_lossy(), &command.cwd).args(command.args.clone());
    let mut process = host
        .spawn_process(&spec)
        .with_context(|| format!("ACP agent を起動できない: {}", command.path.display()))?;
    let stdin = process.take_stdin()?;
    let stdout = process.take_stdout()?;
    let transport = acp::ByteStreams::new(
        blocking::Unblock::new(stdin),
        blocking::Unblock::new(stdout),
    );

    let cwd = command.cwd.clone();
    let outcome = acp::Client
        .builder()
        .connect_with(transport, async move |connection| {
            with_handshake_timeout(
                "ACP initialize",
                connection.send_request(initialize_request()).block_task(),
            )
            .await?;
            // セッションを手動生成する（`start_session` は応答の `config_options` を捨てるため）。
            // NewSessionResponse から config_options を取り出してから attach する。
            let response = connection
                .send_request(v1::NewSessionRequest::new(&cwd))
                .block_task()
                .await?;
            let config_options = response.config_options.clone();
            let mut session = connection.attach_session(response, Vec::new())?;

            // エージェントが広告する権限モード一覧 + 現在モードを UI へ（セレクタを実モードで組む）。
            if let Some(state) = session.modes() {
                let modes = state
                    .available_modes
                    .iter()
                    .map(|mode| (mode.id.to_string(), mode.name.clone()))
                    .collect();
                event_tx
                    .unbounded_send(AgentEvent::Modes {
                        modes,
                        current: state.current_mode_id.to_string(),
                    })
                    .ok();
            }
            // 設定オプション（モデル・思考レベル）を UI へ（あればセレクタを実選択肢に置き換える）。
            if let Some(options) = &config_options {
                event_tx
                    .unbounded_send(AgentEvent::Configs(map_config_options(options)))
                    .ok();
            }

            // ターン中に `session/cancel` を送るための ID（`session` はターン中
            // `read_update` で可変借用されるので、先に控えておく）。
            let session_id = session.session_id().clone();
            // ターン中に届いた非 Cancel コマンド（モデル変更等）を落とさないための待ち行列。
            // ターンが畳まれてから順に処理する。
            let mut deferred: std::collections::VecDeque<SessionCommand> =
                std::collections::VecDeque::new();

            // UI からの指示（prompt / モード変更）を単一チャネルで捌く。
            loop {
                let session_command = match deferred.pop_front() {
                    Some(command) => command,
                    None => match command_rx.next().await {
                        Some(command) => command,
                        // UI が送信ハンドルを drop した＝このセッションは終わり。
                        None => break,
                    },
                };
                let prompt = match session_command {
                    SessionCommand::Prompt(prompt) => prompt,
                    // ターン外の cancel は畳む対象が無いので黙って捨てる。
                    SessionCommand::Cancel => continue,
                    SessionCommand::SetMode(mode_id) => {
                        connection
                            .send_request(v1::SetSessionModeRequest::new(
                                session.session_id().clone(),
                                mode_id,
                            ))
                            .block_task()
                            .await
                            .ok();
                        continue;
                    }
                    SessionCommand::SetConfig {
                        config_id,
                        value_id,
                    } => {
                        // モデル/思考レベル等を変更。応答は更新後の一覧なので UI へ反映する。
                        if let Ok(response) = connection
                            .send_request(v1::SetSessionConfigOptionRequest::new(
                                session.session_id().clone(),
                                config_id,
                                v1::SessionConfigOptionValue::value_id(value_id),
                            ))
                            .block_task()
                            .await
                        {
                            event_tx
                                .unbounded_send(AgentEvent::Configs(map_config_options(
                                    &response.config_options,
                                )))
                                .ok();
                        }
                        continue;
                    }
                };
                if session.send_prompt(prompt).is_err() {
                    // 送信路が死んだ＝以降このセッションは使えない。**終端イベントを必ず出す**
                    // （出さないと UI 側の `running` が落ちず「稼働中」に取り残される）。
                    event_tx
                        .unbounded_send(AgentEvent::Failed(
                            "prompt を送信できませんでした（セッションが閉じています）".into(),
                        ))
                        .ok();
                    break;
                }
                loop {
                    // エージェントの更新と UI のコマンドを**同時に**待つ。こうしないと
                    // ターン中（`read_update` で待っている間）に cancel を受け取れない。
                    // `read_update` はチャネル受信なので途中で future を捨てても取りこぼさない。
                    let turn_event = {
                        use futures::future::FutureExt as _;
                        let update = session.read_update().fuse();
                        let command = command_rx.next().fuse();
                        futures::pin_mut!(update, command);
                        futures::select! {
                            update = update => TurnEvent::Update(update),
                            command = command => TurnEvent::Command(command),
                        }
                    };
                    let update = match turn_event {
                        TurnEvent::Command(Some(SessionCommand::Cancel)) => {
                            // 通知なので応答は無い。エージェントが `StopReason::Cancelled` を
                            // 返してターンを畳む → 下の StopReason 分岐で TurnEnded が流れる。
                            connection
                                .send_notification(v1::CancelNotification::new(session_id.clone()))
                                .ok();
                            continue;
                        }
                        // ターン中のモデル変更等は畳んでから処理する（取りこぼさない）。
                        TurnEvent::Command(Some(other)) => {
                            deferred.push_back(other);
                            continue;
                        }
                        // UI が drop した。ターンを畳んで外側も抜ける。
                        TurnEvent::Command(None) => break,
                        TurnEvent::Update(Ok(update)) => update,
                        TurnEvent::Update(Err(error)) => {
                            event_tx
                                .unbounded_send(AgentEvent::Failed(error.to_string()))
                                .ok();
                            break;
                        }
                    };
                    match update {
                        acp::SessionMessage::SessionMessage(dispatch) => {
                            acp::util::MatchDispatch::new(dispatch)
                                .if_notification(async |notification: v1::SessionNotification| {
                                    match notification.update {
                                        v1::SessionUpdate::AgentMessageChunk(
                                            v1::ContentChunk {
                                                content: v1::ContentBlock::Text(text),
                                                ..
                                            },
                                        ) => {
                                            event_tx
                                                .unbounded_send(AgentEvent::AgentChunk(text.text))
                                                .ok();
                                        }
                                        v1::SessionUpdate::AgentThoughtChunk(
                                            v1::ContentChunk {
                                                content: v1::ContentBlock::Text(text),
                                                ..
                                            },
                                        ) => {
                                            event_tx
                                                .unbounded_send(AgentEvent::ThoughtChunk(text.text))
                                                .ok();
                                        }
                                        v1::SessionUpdate::ToolCall(tool_call) => {
                                            event_tx
                                                .unbounded_send(AgentEvent::ToolStarted(
                                                    ToolCallInfo {
                                                        id: tool_call.tool_call_id.0.to_string(),
                                                        title: Some(tool_call.title),
                                                        kind: Some(map_tool_kind(tool_call.kind)),
                                                        locations: tool_locations(
                                                            &tool_call.locations,
                                                        ),
                                                        diffs: tool_diffs(&tool_call.content),
                                                        output: tool_output(&tool_call.content),
                                                        completed: tool_completed(tool_call.status),
                                                    },
                                                ))
                                                .ok();
                                        }
                                        v1::SessionUpdate::ToolCallUpdate(update) => {
                                            let content =
                                                update.fields.content.as_deref().unwrap_or(&[]);
                                            event_tx
                                                .unbounded_send(AgentEvent::ToolUpdated(
                                                    ToolCallInfo {
                                                        id: update.tool_call_id.0.to_string(),
                                                        title: update.fields.title.clone(),
                                                        kind: update.fields.kind.map(map_tool_kind),
                                                        locations: update
                                                            .fields
                                                            .locations
                                                            .as_deref()
                                                            .map(tool_locations)
                                                            .unwrap_or_default(),
                                                        diffs: tool_diffs(content),
                                                        output: tool_output(content),
                                                        completed: update
                                                            .fields
                                                            .status
                                                            .and_then(tool_completed),
                                                    },
                                                ))
                                                .ok();
                                        }
                                        v1::SessionUpdate::UsageUpdate(usage) => {
                                            event_tx
                                                .unbounded_send(AgentEvent::Usage {
                                                    used: usage.used,
                                                    size: usage.size,
                                                })
                                                .ok();
                                        }
                                        v1::SessionUpdate::CurrentModeUpdate(update) => {
                                            event_tx
                                                .unbounded_send(AgentEvent::ModeChanged(
                                                    update.current_mode_id.to_string(),
                                                ))
                                                .ok();
                                        }
                                        v1::SessionUpdate::ConfigOptionUpdate(update) => {
                                            event_tx
                                                .unbounded_send(AgentEvent::Configs(
                                                    map_config_options(&update.config_options),
                                                ))
                                                .ok();
                                        }
                                        v1::SessionUpdate::Plan(plan) => {
                                            let items = plan
                                                .entries
                                                .iter()
                                                .map(|entry| PlanItem {
                                                    content: entry.content.clone(),
                                                    status: match entry.status {
                                                        v1::PlanEntryStatus::InProgress => {
                                                            PlanStatus::InProgress
                                                        }
                                                        v1::PlanEntryStatus::Completed => {
                                                            PlanStatus::Completed
                                                        }
                                                        // Pending + 将来の未知値は未着手扱い（non_exhaustive）。
                                                        _ => PlanStatus::Pending,
                                                    },
                                                })
                                                .collect();
                                            event_tx.unbounded_send(AgentEvent::Plan(items)).ok();
                                        }
                                        _ => {}
                                    }
                                    Ok(())
                                })
                                .await
                                // エージェントからの **リクエスト**（権限確認）を捌く。応答するまでこの
                                // await は返らない＝ターンは正しくブロックされる（agent 側も待っている）。
                                .if_request(
                                    async |request: v1::RequestPermissionRequest,
                                           responder: acp::Responder<
                                        v1::RequestPermissionResponse,
                                    >| {
                                        handle_permission_request(request, responder, &event_tx)
                                            .await
                                    },
                                )
                                .await
                                .otherwise_ignore()?;
                        }
                        acp::SessionMessage::StopReason(reason) => {
                            // 中断（拒否/キャンセル）は「注意を要する終わり方」として区別する。
                            // 上限到達・未知バリアントは完了扱い（会話は続けられる）。
                            let end = match reason {
                                v1::StopReason::Refusal | v1::StopReason::Cancelled => {
                                    TurnEnd::Interrupted
                                }
                                _ => TurnEnd::Completed,
                            };
                            event_tx
                                .unbounded_send(AgentEvent::TurnEnded { reason: end })
                                .ok();
                            break;
                        }
                        // 将来のバリアント（enum は #[non_exhaustive]）は無視して読み続ける。
                        _ => {}
                    }
                }
            }
            Ok::<(), acp::Error>(())
        })
        .await
        .context("ACP セッションが異常終了");

    drop(process);
    outcome
}

/// `session/request_permission` を UI へ橋渡しして応答する。
/// タイトル・編集差分・選択肢を [`AgentEvent::PermissionRequest`] で流し、UI が選んだ**添字**を
/// `respond` 経由で受け取って `Selected(option_id)` を返す。UI が sender を drop したら `Cancelled`。
/// ACP の `ToolKind` を UI 非依存な [`ToolCallKind`] へ写す（未知値は Other）。
fn map_tool_kind(kind: v1::ToolKind) -> ToolCallKind {
    match kind {
        v1::ToolKind::Read => ToolCallKind::Read,
        v1::ToolKind::Edit => ToolCallKind::Edit,
        v1::ToolKind::Delete => ToolCallKind::Delete,
        v1::ToolKind::Move => ToolCallKind::Move,
        v1::ToolKind::Search => ToolCallKind::Search,
        v1::ToolKind::Execute => ToolCallKind::Execute,
        v1::ToolKind::Think => ToolCallKind::Think,
        v1::ToolKind::Fetch => ToolCallKind::Fetch,
        _ => ToolCallKind::Other,
    }
}

/// ツール内容からファイル編集差分（before/after）を取り出す（権限カードと同じ抽出）。
fn tool_diffs(content: &[v1::ToolCallContent]) -> Vec<PermissionDiff> {
    content
        .iter()
        .filter_map(|item| match item {
            v1::ToolCallContent::Diff(diff) => Some(PermissionDiff {
                path: diff.path.display().to_string(),
                old_text: diff.old_text.clone(),
                new_text: diff.new_text.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// ツール内容から本文テキスト（Bash 出力・Read 概要など）を連結して取り出す。
fn tool_output(content: &[v1::ToolCallContent]) -> Option<String> {
    let mut text = String::new();
    for item in content {
        if let v1::ToolCallContent::Content(block) = item {
            if let v1::ContentBlock::Text(chunk) = &block.content {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&chunk.text);
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

/// ツールが触ったファイルのパス一覧を取り出す。
fn tool_locations(locations: &[v1::ToolCallLocation]) -> Vec<String> {
    locations
        .iter()
        .map(|location| location.path.display().to_string())
        .collect()
}

/// ツールの完了状態を bool へ（成功=Some(true) / 失敗=Some(false) / 進行中=None）。
fn tool_completed(status: v1::ToolCallStatus) -> Option<bool> {
    match status {
        v1::ToolCallStatus::Completed => Some(true),
        v1::ToolCallStatus::Failed => Some(false),
        _ => None,
    }
}

async fn handle_permission_request(
    request: v1::RequestPermissionRequest,
    responder: acp::Responder<v1::RequestPermissionResponse>,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), acp::Error> {
    let title = request
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "ツールの実行許可".to_string());
    // 編集ツールなら差分が載る（diff レビュー用）。
    let diffs: Vec<PermissionDiff> = request
        .tool_call
        .fields
        .content
        .iter()
        .flatten()
        .filter_map(|content| match content {
            v1::ToolCallContent::Diff(diff) => Some(PermissionDiff {
                path: diff.path.display().to_string(),
                old_text: diff.old_text.clone(),
                new_text: diff.new_text.clone(),
            }),
            _ => None,
        })
        .collect();
    let options: Vec<PermissionChoice> = request
        .options
        .iter()
        .map(|option| PermissionChoice {
            label: option.name.clone(),
            kind: match option.kind {
                v1::PermissionOptionKind::AllowOnce => PermissionKind::Allow,
                v1::PermissionOptionKind::AllowAlways => PermissionKind::AllowAlways,
                v1::PermissionOptionKind::RejectOnce => PermissionKind::Reject,
                v1::PermissionOptionKind::RejectAlways => PermissionKind::RejectAlways,
                _ => PermissionKind::Other,
            },
        })
        .collect();

    let (respond_tx, mut respond_rx) = mpsc::unbounded::<usize>();
    event_tx
        .unbounded_send(AgentEvent::PermissionRequest {
            title,
            diffs,
            options,
            respond: respond_tx,
        })
        .ok();

    // ユーザーの決定（選択肢の添字）を待つ。sender を drop されたら None＝キャンセル。
    let chosen = respond_rx.next().await;
    let outcome = match chosen.and_then(|index| request.options.get(index)) {
        Some(option) => v1::RequestPermissionOutcome::Selected(v1::SelectedPermissionOutcome::new(
            option.option_id.clone(),
        )),
        None => v1::RequestPermissionOutcome::Cancelled,
    };
    responder.respond(v1::RequestPermissionResponse::new(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_command_lookup_is_optional() {
        // PATH に無い環境でも None を返すだけ（パニックしない）
        let _ = AgentCommand::claude(".");
        assert!(find_in_path("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn oneshot_maps_default_agent_to_its_cli() {
        // タイトル生成は「既定 Agent の CLI テンプレート」を使う（Claude 決め打ちをやめた）。
        let claude = AgentKind::by_label("Claude Code")
            .and_then(|k| k.oneshot())
            .unwrap();
        assert!(
            claude.contains("claude -p")
                && claude.contains("{prompt}")
                && claude.contains("{excerpt}")
        );
        // codex は agent 実行で stdout が汚いため --output-last-message + cat でクリーンに拾う。
        let codex = AgentKind::by_label("Codex")
            .and_then(|k| k.oneshot())
            .unwrap();
        assert!(
            codex.contains("codex exec")
                && codex.contains("--output-last-message")
                && codex.contains("{out}")
        );
        // 旧 Gemini CLI(→Antigravity/agy) は ACP 非対応で agent 一覧から除外済み。utility 対応は claude/codex のみ。
        // 非対応 agent は None＝タイトルは既定名フォールバック（Claude へ勝手に流さない）。
        assert_eq!(
            AgentKind::by_label("GitHub Copilot").and_then(|k| k.oneshot()),
            None
        );
        assert_eq!(
            AgentKind::by_label("Kimi CLI").and_then(|k| k.oneshot()),
            None
        );
        assert_eq!(
            AgentKind::by_label("Nonexistent").and_then(|k| k.oneshot()),
            None
        );
    }

    #[test]
    fn timeout_passes_fast_response_through() {
        // すぐ返る future は timeout せず値をそのまま通す（正常なハンドシェイクは素通り）。
        let ready = async { Ok::<i32, acp::Error>(42) };
        let result = futures::executor::block_on(with_timeout(
            std::time::Duration::from_secs(5),
            "ACP initialize",
            ready,
        ));
        assert_eq!(result.ok(), Some(42));
    }

    #[test]
    fn timeout_fires_on_silent_hang() {
        // 永遠に返らない future（＝無言ハング）は timeout してエラーになり、理由が message に載る。
        // これが agent_panel で `AgentEvent::Failed` になり、チャットに「エラー: …」として出る。
        let hang = futures::future::pending::<std::result::Result<i32, acp::Error>>();
        let error = futures::executor::block_on(with_timeout(
            std::time::Duration::from_millis(30),
            "ACP initialize",
            hang,
        ))
        .expect_err("無言ハングは timeout エラーになる");
        let message = error.to_string();
        assert!(
            message.contains("ACP initialize") && message.contains("応答しません"),
            "timeout エラーに理由が載る（UI に出る文言）: {message}"
        );
    }

    /// 無言ハングの実プロセス検証: stdin を読まず stdout に何も返さない子（＝ハングした agent）に対し、
    /// connect_and_initialize が [`HANDSHAKE_TIMEOUT`] で必ずエラーを返す（無限に待たない）。ユニット
    /// テストは helper 単体を見るが、これは実パイプ + ACP トランスポート越しでも timer が発火することを見る。
    /// 30 秒かかるので通常は無視。`cargo test -p acp_client -- --ignored --nocapture times_out`
    #[test]
    #[ignore = "HANDSHAKE_TIMEOUT（30 秒）待つ実プロセス検証"]
    fn connect_times_out_on_silent_hang() {
        // `sleep` は stdin を読まず stdout に何も書かない＝プロセスは生きたまま応答しない無言ハング。
        let command = AgentCommand {
            path: find_in_path("sleep").expect("sleep が PATH に無い"),
            args: vec!["120".to_string()],
            cwd: std::env::temp_dir(),
        };
        let started = std::time::Instant::now();
        let result = futures::executor::block_on(connect_and_initialize(&command));
        let elapsed = started.elapsed();
        let error = result.expect_err("無言ハングは timeout エラーになる（無限待ちにならない）");
        assert!(format!("{error:#}").contains("応答しません"), "理由が UI に出る: {error:#}");
        assert!(
            (29..45).contains(&elapsed.as_secs()),
            "HANDSHAKE_TIMEOUT 付近で返る: {elapsed:?}"
        );
    }

    /// 実プロセス検証: claude-agent-acp を起動して initialize が返るか。
    /// `cargo test -p acp_client -- --ignored --nocapture live_initialize`
    #[test]
    #[ignore = "claude-agent-acp（実プロセス）が要る"]
    fn live_initialize() {
        let cwd = std::env::current_dir().expect("cwd");
        let command = AgentCommand::claude(&cwd).expect("claude-agent-acp が PATH に無い");
        let result = futures::executor::block_on(connect_and_initialize(&command));
        println!("initialize 結果: {result:?}");
        assert!(result.is_ok(), "initialize が成功する: {result:?}");
    }

    /// 実環境の CLI status + ACP session probe を一覧確認する。
    /// `cargo test -p acp_client -- --ignored --nocapture live_auth_states`
    #[test]
    #[ignore = "ローカルの vendor CLI / 資格情報を調べる"]
    fn live_auth_states() {
        let cwd = std::env::current_dir().expect("cwd");
        let states = futures::executor::block_on(refresh_agent_auth_states(cwd));
        for (agent, state) in AGENTS.iter().zip(states) {
            println!("{}: {state:?}", agent.label);
        }
    }

    /// 実プロセス検証: 1 プロンプトを送って応答テキストが返るか。
    /// `cargo test -p acp_client -- --ignored --nocapture live_prompt`
    #[test]
    #[ignore = "claude-agent-acp（実プロセス）+ 認証が要る"]
    fn live_prompt() {
        let cwd = std::env::current_dir().expect("cwd");
        let command = AgentCommand::claude(&cwd).expect("claude-agent-acp が PATH に無い");
        let result =
            futures::executor::block_on(prompt_once(&command, "1+1は？ 数字だけで答えて。"));
        println!("prompt 応答: {result:?}");
        let text = result.expect("prompt が成功する");
        assert!(!text.trim().is_empty(), "応答が空でない");
    }

    /// 実プロセス検証: 常駐セッションが prompt を送って **逐次チャンク**を流すか。
    /// `cargo test -p acp_client -- --ignored --nocapture live_stream`
    #[test]
    #[ignore = "claude-agent-acp（実プロセス）+ 認証が要る"]
    fn live_stream() {
        let cwd = std::env::current_dir().expect("cwd");
        let command = AgentCommand::claude(&cwd).expect("claude-agent-acp が PATH に無い");
        let (prompt_tx, prompt_rx) = mpsc::unbounded();
        let (event_tx, mut event_rx) = mpsc::unbounded();
        prompt_tx
            .unbounded_send(SessionCommand::Prompt(
                "3の倍数を小さい順に5個、カンマ区切りだけで答えて。".to_string(),
            ))
            .expect("send");
        drop(prompt_tx); // 送信ハンドルを閉じる → このターン後に run_session は終了する

        let chunks = futures::executor::block_on(async move {
            let session = run_session(command, prompt_rx, event_tx);
            let drain = async move {
                let mut chunks = Vec::new();
                while let Some(event) = event_rx.next().await {
                    match event {
                        AgentEvent::AgentChunk(text) => {
                            print!("{text}");
                            chunks.push(text);
                        }
                        AgentEvent::ThoughtChunk(text) => eprintln!("[think] {text}"),
                        AgentEvent::ToolStarted(info) => {
                            eprintln!("[tool] {}", info.title.unwrap_or_default())
                        }
                        AgentEvent::ToolUpdated(info) => eprintln!(
                            "[tool update] id={} completed={:?} diffs={} output={}",
                            info.id,
                            info.completed,
                            info.diffs.len(),
                            info.output.map(|text| text.lines().count()).unwrap_or(0)
                        ),
                        AgentEvent::Usage { used, size } => eprintln!("[usage] {used}/{size}"),
                        AgentEvent::Modes { modes, current } => {
                            eprintln!("[modes] current={current} available={modes:?}")
                        }
                        AgentEvent::ModeChanged(id) => eprintln!("[mode changed] {id}"),
                        AgentEvent::Configs(configs) => {
                            for config in &configs {
                                eprintln!(
                                    "[config] id={} category={:?} current={} choices={:?}",
                                    config.config_id,
                                    config.category,
                                    config.current,
                                    config
                                        .choices
                                        .iter()
                                        .map(|(_, name)| name)
                                        .collect::<Vec<_>>()
                                );
                            }
                        }
                        AgentEvent::PermissionRequest {
                            title,
                            options,
                            respond,
                            ..
                        } => {
                            eprintln!(
                                "[permission] {title} options={:?}",
                                options.iter().map(|o| &o.label).collect::<Vec<_>>()
                            );
                            respond.unbounded_send(0).ok(); // テストでは先頭を選んで進める
                        }
                        AgentEvent::Plan(items) => eprintln!("[plan] {} items", items.len()),
                        AgentEvent::TurnEnded { reason } => {
                            eprintln!("\n[turn ended: {reason:?}]")
                        }
                        AgentEvent::Failed(error) => eprintln!("[failed] {error}"),
                    }
                }
                chunks
            };
            let (session_result, chunks) = futures::join!(session, drain);
            session_result.expect("session が正常終了する");
            chunks
        });

        println!("\n--- 受信チャンク数: {}", chunks.len());
        assert!(!chunks.is_empty(), "少なくとも 1 つの AgentChunk が来る");
    }
}
