//! acp_client — Claude Code を ACP（Agent Client Protocol）で包む接続層（M4）。
//!
//! ROADMAP M4: `claude-agent-acp` を子プロセスで起動し、ACP で会話する。この層は接続・セッション・
//! プロンプト送信/更新受信までを担い、UI（agent_panel）はチャネル越しに駆動する。
//! まずは **起動 + initialize ハンドシェイク**（＝「繋がる」ことの土台）。session/prompt は継続。
//!
//! 実行時検証: `claude-agent-acp` バイナリ + Claude 認証が要る（実環境で live 検証済み）。

pub mod codex_limits;
pub mod mcp;
pub mod preset;
pub mod registry;
pub mod usage;

use acp::schema::v1;
use acp::schema::ProtocolVersion;
use agent_client_protocol as acp;
use anyhow::{Context as _, Result};
use futures::channel::mpsc;
use futures::{FutureExt, StreamExt};
use host::{CommandSpec, Host, LocalHost};
use std::collections::BTreeMap;
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
    /// サブエージェントの手順なら、それを走らせている親のツール呼び出しの相関 ID（O17）。
    /// Claude は `_meta.claudeCode.parentToolUseId` に載せる（Task / Agent ツールの id）。
    pub parent: Option<String>,
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

/// エージェントが広告する slash コマンド 1 件（ACP `AvailableCommand` の簡約・O2）。
/// composer の `/` 補完が名前・説明・引数ヒントを並べる。呼び出しは `/{name} 引数` の平文 prompt。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// コマンド名（先頭の `/` は含まない）。例: `compact` / Claude の MCP prompt は `mcp:server:cmd` /
    /// Codex の skill は `$name`。
    pub name: String,
    pub description: String,
    /// 引数の入力ヒント（`input: Unstructured { hint }`）。引数を取らないコマンドは `None`。
    pub hint: Option<String>,
}

/// エージェントの「目標」（`/goal`）の状態。Claude / Codex が `SessionInfoUpdate._meta.goal` で送る
/// 拡張の `status` を簡約したもの（知らない値は [`GoalStatus::Other`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
    /// 利用上限・予算上限で止まった（Codex の `usageLimited` / `budgetLimited` → `limited`）。
    Limited,
    Other,
}

/// 目標への操作（O17）。Codex は initialize の `_meta.goal` で操作の入口（`controlMethod`）と使える
/// 操作（`actions`）を広告する。目標を立てる（set）のは `/goal <目的>` の prompt で足りるので扱わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalAction {
    Pause,
    Resume,
    Clear,
}

impl GoalAction {
    fn as_str(self) -> &'static str {
        match self {
            GoalAction::Pause => "pause",
            GoalAction::Resume => "resume",
            GoalAction::Clear => "clear",
        }
    }

    fn parse(action: &str) -> Option<Self> {
        match action {
            "pause" => Some(GoalAction::Pause),
            "resume" => Some(GoalAction::Resume),
            "clear" => Some(GoalAction::Clear),
            _ => None,
        }
    }
}

/// initialize の `_meta.goal` から (操作の入口, 使える操作)。入口は拡張メソッド（`_` で始まる）だけ
/// 受ける（広告を根拠に標準のメソッドを呼ばない）。使える操作が無ければ None。
fn goal_controls(meta: Option<&v1::Meta>) -> Option<(String, Vec<GoalAction>)> {
    let goal = meta?.get("goal")?;
    let method = goal
        .get("controlMethod")?
        .as_str()
        .filter(|method| method.starts_with('_'))?
        .to_string();
    let actions: Vec<GoalAction> = goal
        .get("actions")?
        .as_array()?
        .iter()
        .filter_map(|action| action.as_str().and_then(GoalAction::parse))
        .collect();
    (!actions.is_empty()).then_some((method, actions))
}

/// エージェントが追っている目標（`/goal <objective>` で立つ・O2）。composer の上に 1 行で出す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGoal {
    pub objective: String,
    pub status: GoalStatus,
}

/// Elicitation（選択肢付き質問）の 1 フィールド。ACP の `ElicitationSchema` の 1 プロパティ
/// （単一選択 = enum/oneOf 付き string / 複数選択 = anyOf 付き array）を UI 非依存に簡約したもの。
#[derive(Debug, Clone)]
pub struct ElicitationField {
    /// スキーマ上のプロパティ名（応答 content のキーになる）。
    pub name: String,
    /// 表示ラベル（プロパティの title、無ければ name）。
    pub label: String,
    /// 選択肢。
    pub options: Vec<ElicitationChoice>,
    /// 複数選択か（true = ACP の array プロパティ。応答は `StringArray` で返す）。
    pub multi: bool,
    /// この質問に付いた自由入力の Other 欄のプロパティ名（O17）。あれば UI は選択肢の下に
    /// 入力欄を出し、書いた文字を**この名前**で返す（選択と両方返してよい。どう読むかは
    /// エージェント側: Claude の AskUserQuestion は、単一選択なら「選ばずに書いた = 答え」
    /// 「選んで書いた = 選択に添えるメモ」、複数選択なら選択に足す）。
    pub custom_answer: Option<String>,
}

/// Elicitation の 1 選択肢（ACP `EnumOption` / `enum` 値の簡約）。
#[derive(Debug, Clone)]
pub struct ElicitationChoice {
    /// 応答で返す値（`const`）。
    pub value: String,
    /// 表示名（`title`、`enum` 値なら値そのもの）。
    pub title: String,
    /// 補足説明（あれば）。
    pub description: Option<String>,
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
    /// 会話の累計コスト（`UsageUpdate.cost`・O11）。Claude はターンの結果ごとに**会話全体の累計**
    /// （Claude Code の推定）を送る。ターンの分は UI が前回との差で数える（`session/load` で引き継いだ
    /// 会話の最初の値は、トランスクリプトに残る過去の分を含む）。
    SessionCost { amount: f64, currency: String },
    /// レート制限の知らせ（O11）。Claude は `usage_update._meta["_claude/rateLimit"]` で送ってくる。
    /// アカウント単位の値。1 回の知らせに全部の窓が載るとは限らないので、受け手は窓ごとに差し替える。
    RateLimits(usage::RateLimits),
    /// ターンに使ったトークン（`PromptResponse._meta.quota`・O11）。`TurnEnded` の直前に流す。
    TurnUsage(usage::TurnTokens),
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
        /// ツールの種別（読む / 書く / 実行 / Web …）。エージェントが言わなければ `None`。
        kind: Option<ToolCallKind>,
        /// このツール呼び出しが触る場所（`locations` と差分のパス、それに rawInput の
        /// `file_path` / `path`。重複なし）。Chat モードはこれで「チャットのフォルダか・渡された
        /// ファイルか・それ以外か」を裁く（`chat_core::policy`）。**ACP はツール名を運ばない**ので、
        /// 裁定の材料は `kind` とここだけ。
        paths: Vec<String>,
        diffs: Vec<PermissionDiff>,
        /// Diff の無いツール（Bash/Fetch/MCP 等）の実引数（rawInput）を整形 JSON で。承認前に
        /// 「何が実行されるか」を必ず可視化する（tool poisoning 対策・ACP #1979 / GHSA-f2g4）。
        raw_input: Option<String>,
        options: Vec<PermissionChoice>,
        respond: mpsc::UnboundedSender<usize>,
    },
    /// エージェントの実行プラン全量（`SessionUpdate::Plan`）。UI は常設チェックリストへ置換反映する。
    Plan(Vec<PlanItem>),
    /// エージェントが使える slash コマンドの一覧（`AvailableCommandsUpdate`）。届くたびに**全量置換**
    /// （差分ではない）。`session/new` / `session/load` の直後＝**待機中**に届くのが普通なので、
    /// [`run_session_on`] は待機中も update を読む。
    Commands(Vec<SlashCommand>),
    /// 会話名が変わった（`SessionInfoUpdate.title`）。`None` = エージェントが名前を消した（null）。
    /// 名前を含まない `SessionInfoUpdate`（`_meta` だけの通知）では流さない。Claude はターン終了の
    /// 数秒後（待機中）に送ってくる。
    TitleChanged(Option<String>),
    /// 目標（`/goal`）が変わった（`SessionInfoUpdate._meta.goal`）。`None` = 目標が消えた。
    GoalChanged(Option<AgentGoal>),
    /// このエージェントが受ける目標への操作（O17）。広告したエージェントだけ、セッションが開いた
    /// 直後（SessionStarted の次）に 1 回。
    GoalControls(Vec<GoalAction>),
    /// エージェントが選択肢付きの質問（Elicitation・form）を出した。**選択式フィールドのみ対応**し、
    /// テキスト/数値/真偽を含むフォームは UI へ出さず即 Decline する（下の handler で弾く）。
    /// `respond` に **(name, 選んだ値の並び) の群**を送ると Accept、`None` を送ると Decline。
    /// 単一選択フィールドは要素 1 つ、複数選択は 0 個以上。自由入力の答えは
    /// `(field.custom_answer, [書いた文字])` として同じ群に入れる（O17）。drop で Cancel。
    ElicitationRequest {
        message: String,
        fields: Vec<ElicitationField>,
        respond: mpsc::UnboundedSender<Option<Vec<(String, Vec<String>)>>>,
    },
    /// prompt を**実際にエージェントへ送った**（ターン開始）。UI はこれで `running` を立てる。
    /// 楽観 UI（送信時に立てる）だけだと、生成中に積んで後回し（deferred）になった prompt が
    /// 走る時に誰も running を立て直せず「生成中なのにアイドル表示」になる。ACP の実送信を正とする。
    TurnStarted,
    /// 1 ターン（prompt→応答）が完了した（`StopReason`）。`reason` で正常完了/中断を区別する。
    TurnEnded { reason: TurnEnd },
    /// エラー（接続断・プロトコル異常・起動失敗など）。
    Failed(String),
    /// 失敗ではないが黙って進めたくない知らせ（MCP サーバを渡せなかった等）。transcript に 1 行
    /// 出すだけで、ターン状態（running / auth_required）には触らない。
    Notice(String),
    /// セッションが開いた（`session/new` または `session/load` の直後・Modes/Configs より前）。
    /// `session_id` はエージェント側の会話の鍵。UI はスレッドに控えて、次にこのスレッドの
    /// エージェントを立ち上げ直すとき [`SessionPreferences::resume`] に渡す。
    /// `resumed` = `session/load` で前回の会話を引き継げた（false は新規セッション）。
    SessionStarted {
        session_id: String,
        resumed: bool,
        /// エージェントが `loadSession` を広告している＝このセッションを畳んでも、次に同じ id で
        /// 会話を引き継げる。UI はこれが `true` のスレッドだけ、使っていないエージェントを止めてよい
        /// （広告しないエージェントを止めると会話の文脈が失われる）。
        resumable: bool,
    },
    /// エージェントとの transport が閉じた（プロセス終了・SSH 切断）。**この後セッションは終わる**
    /// （[`run_session_on`] が戻り、イベントチャネルも閉じる）。ターン中なら UI はそのターンを
    /// 畳み、以後の送信は新しいセッションを立ち上げる。待機中にも届く（EOF を待機中も見張るため）。
    SessionLost,
}

/// ターンの終わり方（ACP `StopReason` の簡約）。UI の「完了/中断」の出し分けに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// 正常完了 or 上限到達（`EndTurn` / `MaxTokens` / `MaxTurnRequests`）。
    Completed,
    /// 中断（`Refusal` / `Cancelled`）＝ユーザーの注意を要する終わり方。完了音は鳴らさない。
    Interrupted,
}

/// prompt に添える画像 1 枚（ACP の image ブロック）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptImage {
    /// `image/png` など。
    pub mime_type: String,
    /// 中身（base64）。
    pub data: String,
}

/// UI → 常駐セッションへの指示（[`run_session`] が単一チャネルで受ける）。
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// prompt を送る。
    Prompt(String),
    /// 画像つきの prompt（貼り付けたスクリーンショット・ドロップした画像）。エージェントが
    /// `promptCapabilities.image` を広告していなければ画像は落として、その旨を 1 行知らせる。
    PromptWithImages {
        text: String,
        images: Vec<PromptImage>,
    },
    /// 実行中のターンを中断する（`session/cancel` 通知）。エージェントは
    /// `StopReason::Cancelled` でターンを畳むので、UI には `TurnEnded { Interrupted }` が届く。
    /// **ターン中に受け取れる**必要があるため、ターンループが `read_update` と同時に待つ。
    Cancel,
    /// 権限モードを変更する（`session/set_mode`。引数は mode_id）。
    SetMode(String),
    /// 設定オプション（モデル・思考レベル等）を変更する（`session/set_config_option`）。
    SetConfig { config_id: String, value_id: String },
    /// 目標を一時停止 / 再開 / 取り消す（エージェントが広告した拡張メソッド・O17）。
    /// ターン中でも待たずに送る（目標が自走させているターンを止めたい時こそ使う）。
    Goal(GoalAction),
}

/// ターンループが待つ 3 系統（エージェントからの更新 / UI からのコマンド / prompt 応答）。
/// `select!` の戻り値を所有型にして、`session` と `command_rx` の借用をブロック内で閉じる。
enum TurnEvent {
    Update(Result<acp::SessionMessage, acp::Error>),
    Command(Option<SessionCommand>),
    /// `session/prompt` の応答＝ターンの終端。`Ok` は StopReason、`Err` は API/エージェント側の
    /// 失敗（例: ストリーミング切断）。どちらでもセッション自体は生きている。
    PromptFinished(Result<v1::PromptResponse, acp::Error>),
}

/// 待機中（ターンとターンの間）に待つ 3 系統。エージェントは待機中にも更新を送ってくる
/// （`session/new` 直後のコマンド一覧・ターン後に非同期で付く会話名・自走する目標のターン）。
/// 読まずにおくと SDK のチャネルに溜まり、次の prompt の頭でまとめて流れて遅れる。
enum IdleEvent {
    Update(Result<acp::SessionMessage, acp::Error>),
    Command(Option<SessionCommand>),
    /// transport が閉じた（プロセス終了・SSH 切断）。
    Closed,
}

/// ACP エージェント（claude-agent-acp）の起動設定。
pub struct AgentCommand {
    pub path: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// 起動時に足す環境変数（レジストリの `env` と設定の上書き分）。空が通常。
    pub env: BTreeMap<String, String>,
}

impl AgentCommand {
    /// 既定エージェント（Claude）の起動コマンド。`cwd` はプロジェクトルート。互換用。
    pub fn claude(cwd: impl Into<PathBuf>) -> Option<AgentCommand> {
        AGENTS.first()?.command(cwd)
    }

    /// env 無しで組む（既存経路の短縮形）。
    fn new(path: PathBuf, args: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            path,
            args,
            cwd,
            env: BTreeMap::new(),
        }
    }
}

/// 設定（`settings.json` の `agent_servers.<id>`）から来る起動方法の上書き。
///
/// `settings_core` の型をそのまま持ち込まず、この crate の言葉へ写して受け取る
/// （acp_client は設定スキーマを知らないままにする＝依存方向を増やさない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentOverride {
    /// 起動コマンドを丸ごと差し替える。`None` = 版の解決は通常どおり行い、env だけ足す。
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
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
    /// 公開レジストリ側の id（necoder の `id` とは綴りが違う。例 `copilot` →
    /// `github-copilot-cli`）。`None` = レジストリに項目が無く、`package` だけが頼り。
    registry_id: Option<&'static str>,
    /// npx フォールバック用の npm パッケージ。**レジストリが引けないときの既定値**
    /// （necoder が検証した版。npm 外＝kimi は None）。
    package: Option<&'static str>,
    extra_args: &'static [&'static str],
    /// セットアップ画面の「入れ方」でターミナルに流す導入コマンド（vendor の CLI 本体を入れる）。
    pub install_cmd: &'static str,
    /// セットアップ画面の「ログイン」でターミナルに流す認証コマンド（vendor 自身のログイン導線）。
    /// necoder は鍵を持たず、CLI 側の認証にそのまま乗る（Zed の ACP と同じ流儀）。
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
        registry_id: Some("claude-acp"),
        package: Some("@agentclientprotocol/claude-agent-acp@0.73.0"),
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
        registry_id: Some("codex-acp"),
        package: Some("@agentclientprotocol/codex-acp@1.8.0"),
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
        registry_id: Some("github-copilot-cli"),
        package: Some("@github/copilot@1.0.82"),
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
        registry_id: Some("qwen-code"),
        package: Some("@qwen-code/qwen-code@0.22.3"),
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
        registry_id: Some("opencode"),
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
        registry_id: Some("kimi"),
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
        registry_id: Some("grok-build"),
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

/// [`AgentEvent::Failed`] のメッセージが **認証切れ**（再ログインで直る失敗）かを判定する。
///
/// ACP は認証エラーに専用コードを持たず、`-32603 Internal error` の message に各 Agent が
/// 自由文で載せてくる（例: claude-agent-acp「Internal error: Failed to authenticate: OAuth
/// session expired and could not be refreshed: {"errorKind":"authentication_failed"}」）。
/// 語彙は Agent ごとに違うので、Claude / Codex / Copilot / Gemini 系 CLI の実文言に現れる
/// 定型句を大文字小文字無視で拾う。UI はこれが真なら「エラー: …」の一般表示ではなく
/// 「再ログインが必要」のカード（案内 + 再接続導線）を出し、古いトークンを抱えた
/// プロセスを捨てる。誤検知は「再ログイン案内が余計に出る」だけで、取りこぼしより軽い。
pub fn is_auth_failure(message: &str) -> bool {
    const MARKERS: &[&str] = &[
        "authentication_failed",
        "authentication failed",
        "failed to authenticate",
        "oauth",
        "not logged in",
        "not authenticated",
        "unauthenticated",
        "unauthorized",
        "invalid api key",
        "invalid_api_key",
        "invalid authentication",
        "auth_required",
        "login required",
        "please run /login",
        "please log in",
        "please login",
    ];
    let lowered = message.to_lowercase();
    MARKERS.iter().any(|marker| lowered.contains(marker))
}

/// [`AgentEvent::Failed`] のメッセージが **セッション終了**（同じプロセスへ何を送っても直らない失敗）かを
/// 判定する。claude-agent-acp は SDK の query ストリームが死ぬと以後の `session/prompt` を
/// 「The Claude Agent session has ended. Please start a new session.」で拒み続ける（認証切れ後の
/// resume が失敗した時もここへ落ちる）。UI はこれが真なら送信路を捨て、次の送信で新プロセスを立てる。
pub fn is_session_ended(message: &str) -> bool {
    const MARKERS: &[&str] = &[
        "session has ended",
        "start a new session",
        "session not found",
        "process exited unexpectedly",
    ];
    let lowered = message.to_lowercase();
    MARKERS.iter().any(|marker| lowered.contains(marker))
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
///
/// `disabled`（設定の `disabled_agents`・`AgentKind::id`）のエージェントは子プロセスを起こさず、
/// ファイルだけの軽い判定のまま返す（使わないと決めたものを確かめに行かない・O16）。
pub async fn refresh_agent_auth_states(
    cwd: impl Into<PathBuf>,
    disabled: &[String],
) -> Vec<AgentAuthState> {
    let cwd = cwd.into();
    let initial = detect_configured_agent_states();
    let probes = AGENTS
        .iter()
        .zip(initial.iter().copied())
        .map(|(agent, state)| {
            let cwd = cwd.clone();
            let skip = disabled.iter().any(|id| id == agent.id);
            async move {
                if skip || !agent.cli_installed() {
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

    /// 認証が切れた時にユーザーへ案内する **再ログインコマンド**（ターミナルで打つ 1 行）。
    /// 対話ログインは ACP 越しに代行できない（ブラウザ・コードのやり取りが要る）ため、
    /// necoder は「何を打てばよいか」を示すに留める。**実機で `--help` を確認したものだけ**載せ、
    /// 未確認の Agent は `None`＝「エージェントの CLI で再ログイン」の汎用文で案内する。
    pub fn login_command(&self) -> Option<&'static str> {
        match self.id {
            "claude" => Some("claude auth login"),
            "codex" => Some("codex login"),
            "opencode" => Some("opencode auth login"),
            _ => None,
        }
    }

    /// **設定 → レジストリ → 組み込みカタログ**の順で起動コマンドを決める（本命の入口）。
    ///
    /// - `settings`: `settings.json` の `agent_servers.<id>`。`command` があれば**そこで確定**
    ///   （版の解決もレジストリ参照もしない＝ユーザーが完全に主導権を持つ逃げ道）。
    /// - `registry`: 公開レジストリのキャッシュ。あれば npm パッケージ版はここが正になる。
    ///   necoder のリリースを待たずに版が進むのはこの経路のおかげ。
    /// - どちらも無ければ [`Self::command`]（組み込みカタログ＝necoder が検証した既定値）。
    ///
    /// `env` は**どの経路でも設定の分を最後に足す**（コマンドは差し替えないが環境だけ変えたい、
    /// を `type: "registry"` で表せるようにするため）。
    pub fn resolve_command(
        &self,
        cwd: impl Into<PathBuf>,
        settings: Option<&AgentOverride>,
        registry: Option<&registry::Registry>,
    ) -> Option<AgentCommand> {
        let cwd = cwd.into();
        let settings_env = settings.map(|s| s.env.clone()).unwrap_or_default();

        // 1) 設定でコマンドごと差し替え。PATH 上の名前も絶対パスも受ける。
        if let Some(override_settings) = settings {
            if let Some(command) = &override_settings.command {
                let path = find_in_path(command).unwrap_or_else(|| PathBuf::from(command));
                let mut args = override_settings.args.clone();
                args.extend(self.extra_args.iter().map(|arg| (*arg).to_string()));
                let mut resolved = AgentCommand::new(path, args, cwd);
                resolved.env = settings_env;
                return Some(resolved);
            }
        }

        // 2) レジストリ。npx 経路だけ起動できる（binary 配備は未実装＝黙って落とさず次へ）。
        if let Some(entry) = self
            .registry_id
            .and_then(|id| registry.and_then(|reg| reg.agent(id)))
        {
            if let Some(registry::Launch::Npx(npx)) = entry.launch() {
                // 同じ版が npx のキャッシュに在れば直接起こす（`npm exec` の親プロセスを持たない）。
                // レジストリの起動引数が付いている時は npx の解釈に任せる（取り違えない）。
                let cached = npm_npx_cache_root()
                    .filter(|_| npx.args.is_empty())
                    .and_then(|cache| npx_cached_agent(&cache, &npx.package, self.bin));
                if let Some(bin) = cached {
                    let args = self
                        .extra_args
                        .iter()
                        .map(|arg| (*arg).to_string())
                        .collect();
                    let mut resolved = AgentCommand::new(bin, args, cwd);
                    resolved.env = npx.env.clone();
                    resolved.env.extend(settings_env);
                    return Some(resolved);
                }
                if let Some(npx_path) = find_in_path("npx") {
                    let mut args = vec!["-y".to_string(), bounded_npm_spec(&npx.package)];
                    args.extend(npx.args.iter().cloned());
                    args.extend(self.extra_args.iter().map(|arg| (*arg).to_string()));
                    let mut resolved = AgentCommand::new(npx_path, args, cwd);
                    resolved.env = npx.env.clone();
                    resolved.env.extend(settings_env);
                    return Some(resolved);
                }
            }
        }

        // 3) 組み込みカタログ（レジストリが引けない・オフライン・未登録のとき）。
        let mut resolved = self.command(cwd)?;
        resolved.env.extend(settings_env);
        Some(resolved)
    }

    /// このエージェントの起動コマンドを解決する（組み込みカタログのみ）。
    /// 探索順: (1) PATH の単体バイナリ → (2) Zed の npx キャッシュ(.bin) → (3) `npx <package> <args>`。
    ///
    /// 設定とレジストリまで見るのは [`Self::resolve_command`]。
    pub fn command(&self, cwd: impl Into<PathBuf>) -> Option<AgentCommand> {
        let cwd = cwd.into();
        let extra: Vec<String> = self.extra_args.iter().map(|arg| arg.to_string()).collect();
        // 1) PATH の単体バイナリ
        if let Some(path) = find_in_path(self.bin) {
            return Some(AgentCommand::new(path, extra, cwd));
        }
        // 2) Zed が展開済みの npx キャッシュ（ネット不要）
        if let Some(bin) = zed_cached_agent(self.bin) {
            return Some(AgentCommand::new(bin, extra, cwd));
        }
        // 3) npx フォールバック（npm パッケージがある agent のみ。node/npx が PATH に要る）。
        //    同じ版が npx のキャッシュに在れば直接起こす。
        let package = self.package?;
        if let Some(bin) =
            npm_npx_cache_root().and_then(|cache| npx_cached_agent(&cache, package, self.bin))
        {
            return Some(AgentCommand::new(bin, extra, cwd));
        }
        let npx = find_in_path("npx")?;
        let mut args = vec!["-y".to_string(), bounded_npm_spec(package)];
        args.extend(extra);
        Some(AgentCommand::new(npx, args, cwd))
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
    /// 設定画面の導入/ログイン後ポーリング（変化検知）もこの軽さを前提に毎秒呼ぶ。
    pub fn configured_auth_state(&self) -> AgentAuthState {
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
        Some(AgentCommand::new(
            path,
            self.extra_args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect(),
            cwd,
        ))
    }

    /// initialize → session/new までを短時間だけ実行する。prompt は送らないため利用料は発生せず、
    /// タイムアウト・完了のどちらでも [`host::HostProcess`] の Drop が子を kill/wait する。
    async fn probe_acp_session(&self, cwd: impl Into<PathBuf>) -> bool {
        let Some(command) = self.installed_command(cwd) else {
            return false;
        };
        probe_session(&command, Duration::from_secs(4)).await
    }

    /// 指定 host 上で **設定 → レジストリ → 組み込み**の順に解決する（remote 対応版）。
    ///
    /// remote では設定の `command` と レジストリの npm パッケージ版が効き、探索そのものは
    /// remote 側で行う（remote の認証情報は remote 側のものだけを使う原則は変えない）。
    ///
    /// 戻り値の 3 態を混ぜない: `Ok(Some)` = 見つかった / `Ok(None)` = **探せたが無い**（導入案内へ）/
    /// `Err` = **探せなかった**（SSH 再接続の失敗など。原因文をそのまま出す）。以前は `Err` も
    /// `None` に畳んでいたため、SSH が切れているだけで「claude-agent-acp が見つかりません」と
    /// 誤報していた（2026-09-08 実機）。
    pub fn resolve_command_on(
        &self,
        host: &dyn Host,
        cwd: impl Into<PathBuf>,
        settings: Option<&AgentOverride>,
        registry: Option<&registry::Registry>,
    ) -> Result<Option<AgentCommand>> {
        let cwd = cwd.into();
        if !host.is_remote() {
            return Ok(self.resolve_command(cwd, settings, registry));
        }
        let settings_env = settings.map(|s| s.env.clone()).unwrap_or_default();
        // 設定でコマンドを指定していれば remote でもそれを使う（解決は remote に任せる）。
        if let Some(command) = settings.and_then(|s| s.command.as_deref()) {
            let mut args = settings.map(|s| s.args.clone()).unwrap_or_default();
            args.extend(self.extra_args.iter().map(|arg| (*arg).to_string()));
            let mut resolved = AgentCommand::new(PathBuf::from(command), args, cwd);
            resolved.env = settings_env;
            return Ok(Some(resolved));
        }
        // レジストリの版で npx 経路を組む（remote 上の npx を使う）。
        let registry_package = self
            .registry_id
            .and_then(|id| registry.and_then(|reg| reg.agent(id)))
            .and_then(|entry| match entry.launch() {
                Some(registry::Launch::Npx(npx)) => Some(npx),
                _ => None,
            });
        let Some(mut resolved) = self.command_on_with_package(
            host,
            cwd,
            registry_package.as_ref().map(|npx| npx.package.as_str()),
        )?
        else {
            return Ok(None);
        };
        if let Some(npx) = registry_package {
            resolved.env.extend(npx.env);
        }
        resolved.env.extend(settings_env);
        Ok(Some(resolved))
    }

    /// 指定 host 上で agent を解決する。remote の認証情報は remote 側のものだけを使う。
    /// 戻り値の 3 態は [`Self::resolve_command_on`] と同じ。
    pub fn command_on(
        &self,
        host: &dyn Host,
        cwd: impl Into<PathBuf>,
    ) -> Result<Option<AgentCommand>> {
        self.command_on_with_package(host, cwd, None)
    }

    /// [`Self::command_on`] の本体。`package_override` はレジストリ由来の npm パッケージ
    /// （`None` = 組み込みカタログの `package` を使う）。
    fn command_on_with_package(
        &self,
        host: &dyn Host,
        cwd: impl Into<PathBuf>,
        package_override: Option<&str>,
    ) -> Result<Option<AgentCommand>> {
        let cwd = cwd.into();
        if !host.is_remote() {
            return Ok(self.command(cwd));
        }
        // ここは上の `if !host.is_remote()` で早期 return した後＝**必ずリモート（Linux）**。
        // だから `sh` で正しい。`cfg!(windows)` で `cmd.exe` に振ってはいけない
        // （Windows クライアントからリモート Linux へ cmd.exe を送ることになる・WINDOWS-PORT.md §D3）。
        //
        // `command -v` は読み取り専用なので **再送可**で送る。切断直後の 1 発目は「送ったが結果
        // 不明」で接続が張り直されるが、非冪等扱いだとそこで失敗して次の候補（npx）へ落ちていた。
        let resolve = |binary: &str| -> Result<Option<PathBuf>> {
            let output = host
                .run_command_retry_safe(&CommandSpec::new("sh", &cwd).args([
                    "-lc".to_string(),
                    format!("command -v -- {}", shell_word(binary)),
                ]))
                .with_context(|| format!("remote で {binary} を探せない"))?;
            Ok(output
                .success()
                .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string())))
        };
        let extra: Vec<String> = self.extra_args.iter().map(|arg| arg.to_string()).collect();
        if let Some(path) = resolve(self.bin)? {
            return Ok(Some(AgentCommand::new(path, extra, cwd)));
        }
        let Some(package) = package_override.or(self.package) else {
            return Ok(None);
        };
        let Some(npx) = resolve("npx")? else {
            return Ok(None);
        };
        let mut args = vec!["-y".to_string(), bounded_npm_spec(package)];
        args.extend(extra);
        Ok(Some(AgentCommand::new(npx, args, cwd)))
    }
}

/// npm の指定を**完全一致ピンから上限つき範囲へ**変える（`pkg@1.2.3` → `pkg@0.0.0 - 1.2.3`）。
///
/// 完全一致で固定すると、npm の `min-release-age` を設定している環境では**公開直後の版が
/// 入らない**（その版は「新しすぎる」と拒否され、代わりに落とせる版も指定されていない）。
/// 上限だけ決めておけば、npm が条件を満たす中で最新の版を選べる。
///
/// `<=1.2.3` ではなく**ハイフン記法**なのは Windows のため — `npm.cmd` を PowerShell 経由で
/// 起動すると `<` が入力リダイレクトとして食われる。
///
/// 版を含まない指定（`opencode-ai` ＝常に最新）と、scope 付きパッケージ名の先頭 `@` は触らない。
fn bounded_npm_spec(package: &str) -> String {
    // 先頭の `@` は scope であって版の区切りではない（`@scope/name@1.2.3`）。
    let Some(separator) = package.rfind('@').filter(|index| *index > 0) else {
        return package.to_string();
    };
    let (name, version) = package.split_at(separator);
    let version = &version[1..];
    // 版が空、または既に範囲/タグ指定なら触らない（上限を二重に付けない）。
    if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
        return package.to_string();
    }
    format!("{name}@0.0.0 - {version}")
}

async fn probe_session(command: &AgentCommand, timeout: Duration) -> bool {
    let spec = CommandSpec::new(command.path.to_string_lossy(), &command.cwd)
        .args(command.args.clone())
        .envs(command.env.clone());
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
    paths::home_dir().is_some_and(|home| home.join(relative).exists())
}

fn opencode_auth_exists() -> bool {
    let path = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| paths::home_dir().map(|home| home.join(".local/share")))
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

/// Zed の npx キャッシュ置き場の候補（`<root>/node/cache/_npx`）。
///
/// **これは Zed 自身のディレクトリ規約**なので necoder の `paths` crate は使わない（別アプリの置き場）。
/// 版やプラットフォームで揺れるため、存在するものを順に試す。mac の従来パスを必ず先頭に置く
/// ＝mac では当たった時点で従来と同じ結果になる（WINDOWS-PORT.md §D8）。
/// npm の npx キャッシュ（`~/.npm/_npx`）の場所。`npm_config_cache` があればそちら。
fn npm_npx_cache_root() -> Option<PathBuf> {
    if let Some(cache) = std::env::var_os("npm_config_cache") {
        return Some(PathBuf::from(cache).join("_npx"));
    }
    Some(paths::home_dir()?.join(".npm/_npx"))
}

/// `npx -y <package>@<version>` が**既に入れてある**アダプタの実行ファイル。
///
/// `npx` 経由で起こすと、`npm exec` のプロセスがエージェントの親として生き続ける（実測 48 MB/本・
/// 起動のたびに依存解決で 1 秒前後）。同じ物がキャッシュに在るなら直接起こせば、その両方が要らない。
///
/// 使うのは**指定された版とちょうど同じ版**が入っている時だけ。「上限以下なら何でも」にすると、
/// レジストリが版を上げても古いキャッシュを使い続けて更新されなくなる（無ければ従来どおり npx に
/// 任せる＝npx が新しい版を入れ、次の起動からはそれを直接使う）。
/// Windows は `.cmd` のラッパー経由になるので対象外（従来どおり npx）。
fn npx_cached_agent(cache_root: &Path, package: &str, bin: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    // 先頭の `@` は scope。版の区切りは最後の `@`。
    let separator = package.rfind('@').filter(|index| *index > 0)?;
    let (name, version) = (&package[..separator], &package[separator + 1..]);
    if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
        return None; // 範囲やタグ指定は「どの版か」が決まらない
    }
    for entry in std::fs::read_dir(cache_root).ok()?.flatten() {
        let modules = entry.path().join("node_modules");
        let manifest = modules.join(name).join("package.json");
        let installed = std::fs::read_to_string(&manifest)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|json| json.get("version")?.as_str().map(str::to_string));
        let candidate = modules.join(".bin").join(bin);
        if installed.as_deref() == Some(version) && candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn zed_npx_cache_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = paths::home_dir() {
        roots.push(home.join("Library/Application Support/Zed")); // macOS（従来）
        roots.push(home.join(".local/share/zed")); // Linux
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Zed")); // Windows
    }
    roots
        .into_iter()
        .map(|root| root.join("node/cache/_npx"))
        .collect()
}

/// Zed の npx キャッシュから指定 `.bin/<name>` を探す。node シェバングなので node が PATH に要る。
/// `<cache>/<hash>/node_modules/.bin/<name>`。
fn zed_cached_agent(bin: &str) -> Option<PathBuf> {
    for cache in zed_npx_cache_roots() {
        let Ok(entries) = std::fs::read_dir(&cache) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join("node_modules/.bin").join(bin);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// PATH からバイナリを探す。
pub fn find_in_path(binary: &str) -> Option<PathBuf> {
    // 解決規則は host に集約している。**Windows は拡張子込みでないと見つからない**
    // （ACP の `claude` は Windows では `claude.cmd`）ので、ここで素朴に join してはいけない。
    host::find_in_path(binary)
}

/// 我々（クライアント）の能力を広告する initialize リクエスト。
/// **設定オプション（モデル・思考レベル）** を受け取るには config_options 能力の広告が要る
/// （広告しないとエージェントが `config_options` を送ってこないことがある）。
fn initialize_request() -> v1::InitializeRequest {
    v1::InitializeRequest::new(ProtocolVersion::V1)
        // 我々が誰か（クライアント名 = necoder）をプロトコルの正規経路で伝える。system/user prompt に
        // 「necoder から使われている」と埋め込むのは筋が悪い（Zed もやっていない）。clientInfo が正。
        .client_info(v1::Implementation::new("necoder", env!("CARGO_PKG_VERSION")).title("necoder"))
        .client_capabilities(
            v1::ClientCapabilities::new()
                .session(
                    v1::ClientSessionCapabilities::new().config_options(
                        v1::SessionConfigOptionsCapabilities::new()
                            .boolean(v1::BooleanConfigOptionCapabilities::new()),
                    ),
                )
                // Elicitation（選択肢付き質問）の form モードに対応する旨を広告する。広告しないと
                // エージェントは選択肢 UI を出してこない。unstable API（feature 有効時のみ型が在る）。
                .elicitation(
                    v1::ElicitationCapabilities::new().form(v1::ElicitationFormCapabilities::new()),
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

/// [`find_config_choice`] の結果: どの設定のどの値を送るか、既に current かどうか。
struct ConfigChoice {
    config_id: String,
    value_id: String,
    /// エージェントの current が既にこの値（＝送る必要が無い）。
    is_current: bool,
}

/// スレッドの希望（表示名 or value_id・大文字小文字無視）を、エージェントの広告一覧から引く。
/// 該当カテゴリの広告が無い / 選択肢に無いなら `None`（＝エージェント既定のまま）。
fn find_config_choice(
    configs: &[ConfigOption],
    category: ConfigCategory,
    desired: &str,
) -> Option<ConfigChoice> {
    let config = configs.iter().find(|config| config.category == category)?;
    let (value_id, _) = config.choices.iter().find(|(value_id, name)| {
        name.eq_ignore_ascii_case(desired) || value_id.eq_ignore_ascii_case(desired)
    })?;
    Some(ConfigChoice {
        config_id: config.config_id.clone(),
        value_id: value_id.clone(),
        is_current: *value_id == config.current,
    })
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
    use futures::future::{select, Either};
    let timer = blocking::unblock(move || std::thread::sleep(timeout));
    futures::pin_mut!(future, timer);
    match select(future, timer).await {
        Either::Left((result, _timer)) => result,
        Either::Right(((), _future)) => Err(acp::Error::new(
            i32::from(acp::ErrorCode::InternalError),
            format!(
                "{label} が {} 秒応答しません（エージェントの無言ハング）",
                timeout.as_secs()
            ),
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
            let mut session = connection
                .build_session(&cwd)
                .block_task()
                .start_session()
                .await?;
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

/// セッション起動時にエージェントへ合わせておく「スレッドの希望」（権限モード・モデル・思考量）。
///
/// UI からの後追いコマンド（[`SessionCommand::SetMode`] / [`SessionCommand::SetConfig`]）は遅延起動
/// では **最初の Prompt の後ろ**にキューへ並び、ターン中は deferred に積まれる＝初回ターンには効かない。
/// [`run_session_on`] は `session/new` 直後・最初の prompt を読む前にこれを適用し、適用後の状態を
/// [`AgentEvent::Modes`] / [`AgentEvent::Configs`] として UI へ流す（UI 側の照合が一致し二重送信しない）。
/// 各値は表示名 or id（大文字小文字無視）。エージェントが広告していなければ黙って既定のままにする。
#[derive(Debug, Clone, Default)]
pub struct SessionPreferences {
    /// 権限モード（`session/set_mode`）。
    pub mode: Option<String>,
    /// モデル（`session/set_config_option`・category = Model）。
    pub model: Option<String>,
    /// 思考レベル / effort（`session/set_config_option`・category = ThoughtLevel）。
    pub effort: Option<String>,
    /// 前回このスレッドが使っていた ACP セッション id（[`AgentEvent::SessionStarted`] で控えたもの）。
    /// エージェントが `loadSession` を広告していれば `session/load` で会話を引き継ぐ。広告が無い・
    /// 引き継ぎに失敗した場合は黙って新規セッションにする（SSH 切断・再起動からの復帰・2026-09-08）。
    pub resume: Option<String>,
    /// このセッションでエージェントへ渡す MCP サーバ（解決済み・[`mcp::resolve`] の結果）。
    /// **ACP では渡さない限りエージェントから MCP は 1 つも見えない** — エージェント側の
    /// 設定ファイルに登録してあっても、セッションには出てこない（`mcp` モジュール冒頭）。
    pub mcp_servers: Vec<mcp::McpServerConfig>,
    /// セッションの作り方のプリセット（Chat モード・`docs/CHAT.md` §5）。`session/new` と
    /// `session/load` の**両方**に同じ `_meta` を載せる — 片方だけだと再開した会話から設定が抜ける。
    pub preset: preset::SessionPreset,
}

/// **常駐セッション + 逐次ストリーミング**。エージェントを起動して 1 セッションを開き、`prompt_rx` から
/// 届く各 prompt を送っては `session/update` を [`AgentEvent`] に簡約して `event_tx` へ逐次流す。
/// `prompt_rx` が閉じる（＝送信ハンドルが全て drop）まで常駐し、プロセス・セッションを保持する
/// （同一スレッド内は文脈が続く）。ターン境界は `StopReason` = [`AgentEvent::TurnEnded`]。
pub async fn run_session(
    command: AgentCommand,
    preferences: SessionPreferences,
    command_rx: mpsc::UnboundedReceiver<SessionCommand>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
) -> Result<()> {
    run_session_on(
        LocalHost::shared(),
        command,
        preferences,
        command_rx,
        event_tx,
    )
    .await
}

/// 指定 host 上の常駐 ACP セッション。remote filesystem と agent process を同居させる。
///
/// `preferences` はスレッドが望む権限モード・モデル・思考量（表示名 or id・大文字小文字無視）。
/// **最初の prompt を読む前に**エージェントへ適用する — 遅延起動では UI からの
/// `SetMode` / `SetConfig` コマンドが Prompt の後ろに並ぶため、ここで合わせないと初回ターンが
/// エージェント既定のモード・モデルで走ってしまう（bypass が効かない元凶・JOURNAL 2026-08-28。
/// 「ピルは Opus なのに初回だけ Fable が答える」も同じ構造・2026-09-07）。
pub async fn run_session_on(
    host: Arc<dyn Host>,
    command: AgentCommand,
    preferences: SessionPreferences,
    mut command_rx: mpsc::UnboundedReceiver<SessionCommand>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
) -> Result<()> {
    let spec = CommandSpec::new(command.path.to_string_lossy(), &command.cwd)
        .args(command.args.clone())
        .envs(host::task_environment(host.as_ref(), &command.cwd)?)
        .envs(command.env.clone())
        // プリセットの環境変数が最後＝ユーザーの agent_servers 設定より優先（Chat の持ち込み遮断は設定で外せない）。
        .envs(preferences.preset.env.clone());
    // MCP の stdio サーバはエージェントと同じホストで起動される＝リモートでは接続元のコマンドを
    // 渡しても意味がない。判定はここで取る（下の接続クロージャは `move` で `host` を持ち込まない）。
    let is_remote = host.is_remote();
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
            let initialized = with_handshake_timeout(
                "ACP initialize",
                connection.send_request(initialize_request()).block_task(),
            )
            .await?;
            // 前回のセッション id があり、エージェントが `loadSession` を広告していれば `session/load`
            // で会話を引き継ぐ（SSH 切断・再起動のあとも同じスレッドで続きが話せる）。
            //
            // load 中にエージェントは履歴を `session/update` で**再生**してから応答する。crate は
            // ハンドラ未登録のセッション宛て通知を捨てずに**溜めて attach 時に流す**ので、先に
            // 仮の応答で attach し、load を待つ間に届く update を自分のチャネルで受けて捨てる
            // ＝ transcript に古い会話が二重に載らない。応答と同時に ready だった分も捨て切る。
            // ただし**状態**（コマンド一覧・会話名・目標）は捨てずに流す（[`forward_replayed_state`]）。
            // 再生と同じ窓で届くことがあり、捨てると次の更新まで `/` 補完が空になる。
            // load が失敗したら（id が古い・エージェント側の記録が消えた）新規セッションで続ける。
            // ただし transport が閉じていたら続けても無駄なのでそのまま抜ける。
            // このセッションで使える MCP サーバを ACP の型へ写す。**渡さない限りエージェントからは
            // 1 つも見えない**（`mcp` モジュール冒頭）。渡せなかったものは黙って落とさず知らせる。
            let (mcp_servers, skipped_mcp) = mcp::to_acp_servers(
                &preferences.mcp_servers,
                &initialized.agent_capabilities.mcp_capabilities,
                is_remote,
            );
            for (name, reason) in &skipped_mcp {
                let message = format!(
                    "MCP サーバ「{name}」を渡せませんでした: {}",
                    reason.describe()
                );
                eprintln!("{message}");
                event_tx.unbounded_send(AgentEvent::Notice(message)).ok();
            }

            // プリセットの `_meta`。new と load で同じ物を使う（上の `preset` のコメント参照）。
            let session_meta = preferences.preset.to_meta();

            let accepts_images = initialized.agent_capabilities.prompt_capabilities.image;
            let can_load = initialized.agent_capabilities.load_session;
            let resume_id = preferences.resume.clone().filter(|_| can_load);
            let mut resumed_session = None;
            if let Some(previous) = resume_id {
                use futures::future::FutureExt as _;
                let mut session = connection
                    .attach_session(v1::NewSessionResponse::new(previous.clone()), Vec::new())?;
                let load = connection
                    .send_request(
                        v1::LoadSessionRequest::new(previous.clone(), &cwd)
                            .mcp_servers(mcp_servers.clone())
                            .meta(session_meta.clone()),
                    )
                    .block_task()
                    .fuse();
                futures::pin_mut!(load);
                let loaded = loop {
                    let update = session.read_update().fuse();
                    futures::pin_mut!(update);
                    futures::select_biased! {
                        update = update => match update {
                            // 履歴の再生。本文は捨て、状態だけ流す。
                            Ok(replayed) => {
                                forward_replayed_state(replayed, &event_tx).await;
                                continue;
                            }
                            Err(error) => break Err(error),
                        },
                        result = load => break result,
                    }
                };
                while let Some(Ok(replayed)) = session.read_update().now_or_never() {
                    forward_replayed_state(replayed, &event_tx).await;
                }
                match loaded {
                    Ok(loaded) => {
                        resumed_session = Some((session, loaded.modes, loaded.config_options));
                    }
                    Err(error) if acp::is_incoming_transport_closed(&error) => return Err(error),
                    Err(error) => {
                        eprintln!("ACP session/load に失敗（新規セッションで続行）: {error}");
                        drop(session); // 仮のハンドラを外してから session/new へ
                    }
                }
            }
            let resumed = resumed_session.is_some();
            let (mut session, modes_state, config_options) = match resumed_session {
                Some(resumed_session) => resumed_session,
                None => {
                    // セッションを手動生成する（`start_session` は応答の `config_options` を捨てるため）。
                    // NewSessionResponse から config_options を取り出してから attach する。
                    let response = connection
                        .send_request(
                            v1::NewSessionRequest::new(&cwd)
                                .mcp_servers(mcp_servers)
                                .meta(session_meta),
                        )
                        .block_task()
                        .await?;
                    let modes_state = response.modes.clone();
                    let config_options = response.config_options.clone();
                    let session = connection.attach_session(response, Vec::new())?;
                    (session, modes_state, config_options)
                }
            };
            event_tx
                .unbounded_send(AgentEvent::SessionStarted {
                    session_id: session.session_id().to_string(),
                    resumed,
                    resumable: can_load,
                })
                .ok();
            // 目標への操作の入口（Codex の `_meta.goal`・O17）。広告した時だけ流す（UI は
            // SessionStarted で前の操作を消すので、広告が無ければ操作は出ない）。
            let goal_control = goal_controls(initialized.meta.as_ref());
            if let Some((_, actions)) = goal_control.as_ref() {
                event_tx
                    .unbounded_send(AgentEvent::GoalControls(actions.clone()))
                    .ok();
            }
            // 目標への操作を送る（応答は待たない・結果は `_meta.goal` の更新で届く）。
            let session_id_text = session.session_id().to_string();
            let send_goal_action = |action: GoalAction| {
                let Some((method, _)) = goal_control.as_ref() else {
                    return;
                };
                let params = serde_json::json!({
                    "sessionId": session_id_text,
                    "action": action.as_str(),
                });
                match acp::UntypedMessage::new(method, params) {
                    Ok(message) => connection.send_request(message).detach(),
                    Err(error) => eprintln!("目標の操作を組めない: {error}"),
                }
            };

            // エージェントが広告する権限モード一覧 + 現在モードを UI へ（セレクタを実モードで組む）。
            // 希望モードが広告に在れば**ここで**（初回 prompt より前に）set_mode し、UI へは
            // 適用後の current を流す＝UI 側の照合が一致して二重送信にならない。
            let advertised = modes_state.as_ref().map(|state| {
                let modes: Vec<(String, String)> = state
                    .available_modes
                    .iter()
                    .map(|mode| (mode.id.to_string(), mode.name.clone()))
                    .collect();
                (modes, state.current_mode_id.to_string())
            });
            if let Some((modes, mut current)) = advertised {
                let desired_id = preferences.mode.as_deref().and_then(|desired| {
                    modes
                        .iter()
                        .find(|(id, _)| id == desired)
                        .map(|(id, _)| id.clone())
                });
                if let Some(desired_id) = desired_id {
                    if desired_id != current
                        && connection
                            .send_request(v1::SetSessionModeRequest::new(
                                session.session_id().clone(),
                                desired_id.clone(),
                            ))
                            .block_task()
                            .await
                            .is_ok()
                    {
                        current = desired_id;
                    }
                }
                event_tx
                    .unbounded_send(AgentEvent::Modes { modes, current })
                    .ok();
            }
            // 設定オプション（モデル・思考レベル）も**初回 prompt より前に**スレッドの希望へ合わせる。
            // UI は `Configs` を受けてから `SetConfig` を送るが、遅延起動ではそれが Prompt の後ろに
            // 並び、ターン中は deferred に積まれる＝初回ターンがエージェント既定モデルで走る
            // （ピルは Opus・応答は Fable、の正体。モードの bypass レースと同じ構造）。
            // 広告に無い希望は送らない（UI が広告 current を採用してピルを合わせる）。
            // 適用後の一覧を UI へ流せば、UI 側の照合が一致して二重送信にならない。
            if let Some(options) = &config_options {
                let mut configs = map_config_options(options);
                for (category, desired) in [
                    (ConfigCategory::Model, preferences.model.as_deref()),
                    (ConfigCategory::ThoughtLevel, preferences.effort.as_deref()),
                ] {
                    let Some(desired) = desired else {
                        continue;
                    };
                    let Some(choice) = find_config_choice(&configs, category, desired) else {
                        continue;
                    };
                    if choice.is_current {
                        continue;
                    }
                    if let Ok(response) = connection
                        .send_request(v1::SetSessionConfigOptionRequest::new(
                            session.session_id().clone(),
                            choice.config_id,
                            v1::SessionConfigOptionValue::value_id(choice.value_id),
                        ))
                        .block_task()
                        .await
                    {
                        configs = map_config_options(&response.config_options);
                    }
                }
                event_tx.unbounded_send(AgentEvent::Configs(configs)).ok();
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
                    None => {
                        // 待機中も transport の EOF を見張る。command だけを待つと、SSH 切断で
                        // エージェントが消えても次の送信まで気付けず、送信して初めて
                        // 「Incoming transport closed」になる（2026-09-08 の報告の片割れ）。
                        //
                        // エージェントからの更新も待機中に読む（ターン中と同じ道を通す）。コマンド
                        // 一覧は `session/new` の直後、会話名はターン終了の数秒後に届くのが普通で、
                        // 読まずにいると次の prompt の頭まで UI に届かない（O2）。更新を先に読むのは
                        // ターン中と同じ理由（送った prompt より前に届いた更新を後回しにしない）。
                        use futures::future::FutureExt as _;
                        let idle_event = {
                            let update = session.read_update().fuse();
                            let next_command = command_rx.next().fuse();
                            let closed = connection.incoming_closed().fuse();
                            futures::pin_mut!(update, next_command, closed);
                            futures::select_biased! {
                                update = update => IdleEvent::Update(update),
                                command = next_command => IdleEvent::Command(command),
                                _ = closed => IdleEvent::Closed,
                            }
                        };
                        match idle_event {
                            IdleEvent::Update(Ok(message)) => {
                                handle_session_message(message, &event_tx).await?;
                                continue;
                            }
                            IdleEvent::Update(Err(error)) => {
                                if acp::is_incoming_transport_closed(&error) {
                                    event_tx.unbounded_send(AgentEvent::SessionLost).ok();
                                    break;
                                }
                                // 更新のチャネルが壊れた（セッションが持つ限り起きない）。読み直しても
                                // 同じ失敗が即返るだけなので、空回りさせずにセッションを畳む。
                                return Err(error);
                            }
                            IdleEvent::Command(Some(command)) => command,
                            // UI が送信ハンドルを drop した＝このセッションは終わり。
                            IdleEvent::Command(None) => break,
                            IdleEvent::Closed => {
                                // 閉じる直前に届いていた更新（会話名など）は取りこぼさず流してから畳む。
                                while let Some(Ok(message)) = session.read_update().now_or_never() {
                                    if let Err(error) =
                                        handle_session_message(message, &event_tx).await
                                    {
                                        eprintln!("切断直前の更新を処理できない: {error}");
                                    }
                                }
                                event_tx.unbounded_send(AgentEvent::SessionLost).ok();
                                break;
                            }
                        }
                    }
                };
                let (prompt, images) = match session_command {
                    SessionCommand::Prompt(prompt) => (prompt, Vec::new()),
                    SessionCommand::PromptWithImages { text, images } => (text, images),
                    // ターン外の cancel は畳む対象が無いので黙って捨てる。
                    SessionCommand::Cancel => continue,
                    SessionCommand::Goal(action) => {
                        send_goal_action(action);
                        continue;
                    }
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
                // `Session::send_prompt` は使わない: あれは応答ハンドラを接続内タスクとして spawn
                // するため、エージェントがエラー応答を返す（例: API のストリーミング切断）と
                // タスクアクター経由で接続ドライバごと落ち、「ACP セッションが異常終了」になる。
                // 自前送信して応答をターンループで待てば、失敗を**そのターンだけ**に留められる
                // （応答が返せている＝接続もプロセスも生きているので、セッションを維持して再送できる）。
                let prompt_response = connection
                    .send_request(v1::PromptRequest::new(
                        session_id.clone(),
                        prompt_blocks(prompt, images, accepts_images, &event_tx),
                    ))
                    .block_task()
                    .fuse();
                futures::pin_mut!(prompt_response);
                // ここからが本当のターン開始。deferred から走った 2 本目も含め、UI に
                // running を立てさせる（楽観 UI の取りこぼしを塞ぐ）。TurnEnded と対になる。
                event_tx.unbounded_send(AgentEvent::TurnStarted).ok();
                loop {
                    // エージェントの更新・UI のコマンド・prompt 応答を**同時に**待つ。こうしないと
                    // ターン中（`read_update` で待っている間）に cancel を受け取れない。
                    // `read_update` はチャネル受信なので途中で future を捨てても取りこぼさない。
                    // `select_biased!` で更新をコマンド・応答より先に読む: 応答（終端）が届いた
                    // 時点で未処理の update がチャネルに残っていることがあり（応答は stream 上
                    // 最後だが、両者が同時に ready になる）、公平 select だと末尾のチャンクを
                    // 取りこぼす。
                    let turn_event = {
                        use futures::future::FutureExt as _;
                        let update = session.read_update().fuse();
                        let command = command_rx.next().fuse();
                        futures::pin_mut!(update, command);
                        futures::select_biased! {
                            update = update => TurnEvent::Update(update),
                            command = command => TurnEvent::Command(command),
                            result = prompt_response => TurnEvent::PromptFinished(result),
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
                        // 目標の操作は待たずに送る（目標が自走させているターンを止めたい時こそ使う）。
                        TurnEvent::Command(Some(SessionCommand::Goal(action))) => {
                            send_goal_action(action);
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
                            if acp::is_incoming_transport_closed(&error) {
                                event_tx.unbounded_send(AgentEvent::SessionLost).ok();
                                return Ok(());
                            }
                            event_tx
                                .unbounded_send(AgentEvent::Failed(error.to_string()))
                                .ok();
                            break;
                        }
                        // ターンの終端。中断（拒否/キャンセル）は「注意を要する終わり方」として
                        // 区別する。上限到達・未知バリアントは完了扱い（会話は続けられる）。
                        TurnEvent::PromptFinished(Ok(response)) => {
                            let end = match response.stop_reason {
                                v1::StopReason::Refusal | v1::StopReason::Cancelled => {
                                    TurnEnd::Interrupted
                                }
                                _ => TurnEnd::Completed,
                            };
                            // このターンに使ったトークン（O11）。UI は TurnEnded でコストと一緒に台帳へ書く。
                            if let Some(tokens) = response
                                .meta
                                .as_ref()
                                .and_then(usage::turn_tokens_from_quota)
                            {
                                event_tx.unbounded_send(AgentEvent::TurnUsage(tokens)).ok();
                            }
                            event_tx
                                .unbounded_send(AgentEvent::TurnEnded { reason: end })
                                .ok();
                            break;
                        }
                        // prompt がエラー応答で終わった（例: 「Connection closed mid-response」＝
                        // API のストリーミング切断）。Failed は UI 側で running を落として
                        // transcript にエラーを出す終端イベント。このターンだけ畳み、外側ループへ
                        // 戻ってセッションを維持する（ユーザーは同じスレッドで再送できる）。
                        TurnEvent::PromptFinished(Err(error)) => {
                            if acp::is_incoming_transport_closed(&error) {
                                // エージェントが消えた（プロセス終了・SSH 切断）。このセッションは
                                // 二度と応答しない（crate は EOF 後の request を即座に同じエラーで
                                // 落とす）ので、ターンだけでなくセッションごと畳む。維持すると
                                // 送るたびに「Incoming transport closed」が返り続ける
                                // （2026-09-08 の報告）。UI は SessionLost で送信路を捨て、
                                // 次の送信で立ち上げ直す。
                                event_tx.unbounded_send(AgentEvent::SessionLost).ok();
                                return Ok(());
                            }
                            event_tx
                                .unbounded_send(AgentEvent::Failed(error.to_string()))
                                .ok();
                            break;
                        }
                    };
                    // 通知（本文・ツール・状態）とリクエスト（権限確認・Elicitation）を捌く。リクエストは
                    // 応答するまで返らない＝ターンは正しくブロックされる（agent 側も待っている）。
                    handle_session_message(update, &event_tx).await?;
                }
            }
            Ok::<(), acp::Error>(())
        })
        .await
        .context("ACP セッションが異常終了");

    drop(process);
    outcome
}

/// エージェントからの 1 通（`session/update` 通知・権限確認・Elicitation）を捌く。**ターン中と
/// 待機中で同じ道**を通す — 待機中に届いた更新も、ターン中と同じ写像（[`session_update_events`]）で
/// UI へ流れる。リクエストは応答するまで返らない＝呼び出し側のループもそのぶん止まる
/// （エージェント側も待っている）。
async fn handle_session_message(
    message: acp::SessionMessage,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), acp::Error> {
    let acp::SessionMessage::SessionMessage(dispatch) = message else {
        // StopReason は `send_prompt` 経由でしか流れず、ここは自前送信（PromptFinished で終端を
        // 受ける）なので届かない。将来のバリアント（enum は #[non_exhaustive]）ともども無視する。
        return Ok(());
    };
    acp::util::MatchDispatch::new(dispatch)
        .if_notification(async |notification: v1::SessionNotification| {
            session_update_events(notification.update, |event| {
                event_tx.unbounded_send(event).ok();
            });
            Ok(())
        })
        .await
        // エージェントからの **リクエスト**（権限確認）を捌く。応答するまでこの await は返らない。
        .if_request(
            async |request: v1::RequestPermissionRequest,
                   responder: acp::Responder<v1::RequestPermissionResponse>| {
                handle_permission_request(request, responder, event_tx).await
            },
        )
        .await
        // Elicitation（選択肢付き質問）。権限確認と同じく応答するまで await は返らない。
        .if_request(
            async |request: v1::CreateElicitationRequest,
                   responder: acp::Responder<v1::CreateElicitationResponse>| {
                handle_elicitation_request(request, responder, event_tx).await
            },
        )
        .await
        .otherwise_ignore()
}

/// `session/load` が再生する履歴のうち、**状態**（[`is_session_state_event`]）だけを UI へ流す。
/// 本文・ツール呼び出しは transcript に二重に載るので捨てる。リクエストは再生中には来ない想定で、
/// 来ても従来どおり応答しない（再生を捨てていた頃と同じ扱い）。
async fn forward_replayed_state(
    message: acp::SessionMessage,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
) {
    let acp::SessionMessage::SessionMessage(dispatch) = message else {
        return;
    };
    let result = acp::util::MatchDispatch::new(dispatch)
        .if_notification(async |notification: v1::SessionNotification| {
            session_update_events(notification.update, |event| {
                if is_session_state_event(&event) {
                    event_tx.unbounded_send(event).ok();
                }
            });
            Ok(())
        })
        .await
        .otherwise_ignore();
    if let Err(error) = result {
        eprintln!("session/load の再生中の更新を読めない: {error}");
    }
}

/// 会話の本文ではなくセッションの**状態**を運ぶイベントか（再生中も捨てない物）。
fn is_session_state_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::Commands(_) | AgentEvent::TitleChanged(_) | AgentEvent::GoalChanged(_)
    )
}

/// ACP の `SessionUpdate` 1 件を UI 向けの [`AgentEvent`] へ簡約して `emit` に渡す（UI に出さない
/// 種類は何も渡さない）。ターン中・待機中・`session/load` の再生で同じ写像を使う。
/// 1 件から 2 つ出ることがある（会話名と目標が同じ `SessionInfoUpdate` に載った時）。
fn session_update_events(update: v1::SessionUpdate, mut emit: impl FnMut(AgentEvent)) {
    match update {
        v1::SessionUpdate::AgentMessageChunk(v1::ContentChunk {
            content: v1::ContentBlock::Text(text),
            ..
        }) => emit(AgentEvent::AgentChunk(text.text)),
        v1::SessionUpdate::AgentThoughtChunk(v1::ContentChunk {
            content: v1::ContentBlock::Text(text),
            ..
        }) => emit(AgentEvent::ThoughtChunk(text.text)),
        v1::SessionUpdate::ToolCall(tool_call) => emit(AgentEvent::ToolStarted(ToolCallInfo {
            id: tool_call.tool_call_id.0.to_string(),
            title: Some(tool_call.title),
            kind: Some(map_tool_kind(tool_call.kind)),
            locations: tool_locations(&tool_call.locations),
            diffs: tool_diffs(&tool_call.content),
            output: tool_output(&tool_call.content),
            completed: tool_completed(tool_call.status),
            parent: subagent_parent(tool_call.meta.as_ref()),
        })),
        v1::SessionUpdate::ToolCallUpdate(update) => {
            let content = update.fields.content.as_deref().unwrap_or(&[]);
            emit(AgentEvent::ToolUpdated(ToolCallInfo {
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
                completed: update.fields.status.and_then(tool_completed),
                parent: subagent_parent(update.meta.as_ref()),
            }));
        }
        v1::SessionUpdate::UsageUpdate(update) => {
            emit(AgentEvent::Usage {
                used: update.used,
                size: update.size,
            });
            // 会話の累計コスト（Claude はターンの結果の分にだけ付く）。負・NaN は捨てる。
            if let Some(cost) = update.cost {
                if cost.amount.is_finite() && cost.amount >= 0.0 {
                    emit(AgentEvent::SessionCost {
                        amount: cost.amount,
                        currency: cost.currency,
                    });
                }
            }
            // Claude のレート制限（`rate_limit_event` の中継）。
            if let Some(limits) = update
                .meta
                .as_ref()
                .and_then(|meta| meta.get("_claude/rateLimit"))
                .and_then(usage::rate_limits_from_claude)
            {
                emit(AgentEvent::RateLimits(limits));
            }
        }
        v1::SessionUpdate::CurrentModeUpdate(update) => {
            emit(AgentEvent::ModeChanged(update.current_mode_id.to_string()))
        }
        v1::SessionUpdate::ConfigOptionUpdate(update) => emit(AgentEvent::Configs(
            map_config_options(&update.config_options),
        )),
        v1::SessionUpdate::Plan(plan) => {
            let items = plan
                .entries
                .iter()
                .map(|entry| PlanItem {
                    content: entry.content.clone(),
                    status: match entry.status {
                        v1::PlanEntryStatus::InProgress => PlanStatus::InProgress,
                        v1::PlanEntryStatus::Completed => PlanStatus::Completed,
                        // Pending + 将来の未知値は未着手扱い（non_exhaustive）。
                        _ => PlanStatus::Pending,
                    },
                })
                .collect();
            emit(AgentEvent::Plan(items));
        }
        // 一覧は毎回全量で届く（差分ではない）＝そのまま置き換えに使える形で渡す。
        v1::SessionUpdate::AvailableCommandsUpdate(update) => emit(AgentEvent::Commands(
            update
                .available_commands
                .into_iter()
                .map(slash_command_from)
                .collect(),
        )),
        v1::SessionUpdate::SessionInfoUpdate(update) => {
            // `title` は「無い（未定義）」と「消した（null）」が別。`_meta` だけの通知（Claude の
            // ファイル変更報告・目標など）で名前を消してはいけないので、未定義は何も出さない。
            match update.title {
                acp::schema::MaybeUndefined::Value(title) => {
                    emit(AgentEvent::TitleChanged(Some(title)))
                }
                acp::schema::MaybeUndefined::Null => emit(AgentEvent::TitleChanged(None)),
                acp::schema::MaybeUndefined::Undefined => {}
            }
            if let Some(goal) = update.meta.as_ref().and_then(|meta| meta.get("goal")) {
                emit(AgentEvent::GoalChanged(goal_from_meta(goal)));
            }
        }
        _ => {}
    }
}

/// ACP の `AvailableCommand` を [`SlashCommand`] へ。入力の種類は今は Unstructured（ヒント文字列）だけ。
fn slash_command_from(command: v1::AvailableCommand) -> SlashCommand {
    let hint = match command.input {
        Some(v1::AvailableCommandInput::Unstructured(input)) => {
            Some(input.hint).filter(|hint| !hint.trim().is_empty())
        }
        _ => None,
    };
    SlashCommand {
        name: command.name,
        description: command.description,
        hint,
    }
}

/// `_meta.goal` の値を [`AgentGoal`] へ。null・目的文の無い物は「目標なし」（`None`）。
/// 形は Claude（`claude-agent-acp` の goal 拡張）と Codex（`codex-acp`）で共通の
/// `{ objective, status, … }`。
fn goal_from_meta(goal: &serde_json::Value) -> Option<AgentGoal> {
    let objective = goal.get("objective")?.as_str()?.trim();
    if objective.is_empty() {
        return None;
    }
    let status = match goal.get("status").and_then(serde_json::Value::as_str) {
        Some("active") | None => GoalStatus::Active,
        Some("paused") => GoalStatus::Paused,
        Some("blocked") => GoalStatus::Blocked,
        Some("complete") => GoalStatus::Complete,
        Some("limited") => GoalStatus::Limited,
        Some(_) => GoalStatus::Other,
    };
    Some(AgentGoal {
        objective: objective.to_string(),
        status,
    })
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

/// prompt を ACP の content ブロック列へ。画像は本文の**前**に置く（「この画像について…」と
/// 続く文が自然に読める順）。受け取れないエージェントには送らず、黙って落とさずに知らせる。
///
/// **slash コマンド（`/compact` 等）は本文 1 ブロックだけで送ること**（画像も添付も付けない。
/// 付けないのは UI 側＝agent_panel の責務）。エージェントごとに「コマンドとして読むブロック」が
/// 違い、複数ブロックでは両立しない: Claude Code は**最後の**ブロック（CLI の入力処理）、
/// `claude-agent-acp` の一部判定と `codex-acp` は**先頭の**ブロックを見る。1 ブロックなら両方に効く。
fn prompt_blocks(
    text: String,
    images: Vec<PromptImage>,
    accepts_images: bool,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
) -> Vec<v1::ContentBlock> {
    let mut blocks: Vec<v1::ContentBlock> = Vec::new();
    if accepts_images {
        blocks.extend(images.into_iter().map(|image| {
            v1::ContentBlock::Image(v1::ImageContent::new(image.data, image.mime_type))
        }));
    } else if !images.is_empty() {
        event_tx
            .unbounded_send(AgentEvent::Notice(
                "このエージェントは画像を受け取れないため、画像を除いて送りました".to_string(),
            ))
            .ok();
    }
    blocks.push(text.into());
    blocks
}

/// 権限リクエストが触る場所を、取れる所から全部集める（重複なし・出た順）。
///
/// `locations` が正だが、アダプタによっては権限リクエストの時点で空のまま来る（検索系は
/// 場所を rawInput の `path` にしか書かない）。取りこぼすと「場所が分からないので素通し」に
/// なるので、差分のパスと rawInput のよくあるキーも見る。
fn permission_paths(fields: &v1::ToolCallUpdateFields, diffs: &[PermissionDiff]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut push = |path: String| {
        if !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
    };
    for location in fields.locations.iter().flatten() {
        push(location.path.display().to_string());
    }
    for diff in diffs {
        push(diff.path.clone());
    }
    if let Some(serde_json::Value::Object(input)) = &fields.raw_input {
        for key in ["file_path", "path", "notebook_path"] {
            if let Some(serde_json::Value::String(path)) = input.get(key) {
                push(path.clone());
            }
        }
    }
    paths
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
    let kind = request.tool_call.fields.kind.map(map_tool_kind);
    let paths = permission_paths(&request.tool_call.fields, &diffs);
    // Diff の無いツール（Bash/Fetch/MCP）の実引数を承認前に必ず見せる（ACP #1979 / GHSA-f2g4）。
    // 編集系（Diff あり）は差分表示で内容が見えるので冗長回避のため省く。
    let raw_input = if diffs.is_empty() {
        request
            .tool_call
            .fields
            .raw_input
            .as_ref()
            .and_then(|value| serde_json::to_string_pretty(value).ok())
    } else {
        None
    };
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
            kind,
            paths,
            diffs,
            raw_input,
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

/// AskUserQuestion 系ブリッジが「自由入力の Other 欄」に付ける `_meta` の印。ベンダ接頭辞を持たない
/// 共有キーで、Claude / Codex など複数のブリッジが同じ印を使う取り決めになっている。
/// この印が付いた選択肢無し string は**選択欄の付属品**なので、非対応判定ではなく読み飛ばす。
const CUSTOM_ANSWER_META_KEY: &str = "_askUserQuestionCustomAnswer";

/// ACP の Elicitation フォームを UI 非依存な [`ElicitationField`] 群へ簡約する。
/// **全プロパティが選択式（単一選択 = `enum` / `oneOf` 付き string、複数選択 = `anyOf` / `enum` 付き
/// array）の時だけ** `Some` を返す。テキスト・数値・真偽を含むフォームは、この UI では正しく入力を
/// 返せないので `None`＝非対応にする（呼び出し側が Decline する）。「選ばせる」に絞ることで、
/// 押せば答えになるカードだけを出す。
///
/// 例外は [`CUSTOM_ANSWER_META_KEY`] 印の Other 欄だけ。Claude Code の AskUserQuestion は
/// 質問ごとに「選択肢 + Other 欄」の 2 プロパティを送るため、Other 欄を非対応扱いにすると
/// **質問そのものが Decline されて UI に出ない**（＝エージェントの質問に答えられない）。
/// 印つき Other 欄は独立したフィールドにせず、付属先の選択欄の [`ElicitationField::custom_answer`]
/// に結び付ける（O17・UI は選択肢の下に入力欄を出す）。付属先が見つからない Other 欄は任意入力
/// なので読み飛ばす。
fn simplify_elicitation_form(schema: &v1::ElicitationSchema) -> Option<Vec<ElicitationField>> {
    if schema.properties.is_empty() {
        return None;
    }
    let mut fields = Vec::new();
    // 付属先の質問（選択欄の名前）→ Other 欄の名前。プロパティは名前順に来るので、選択欄より
    // 先に Other 欄が来ても結べるよう、最後にまとめて結ぶ。
    let mut custom_answers = std::collections::BTreeMap::new();
    for (name, property) in &schema.properties {
        let (options, multi, title) = match property {
            v1::ElicitationPropertySchema::String(string_schema) => {
                if is_custom_answer_field(string_schema) {
                    // 選択欄に付属する任意の Other 欄 — 独立した欄にはせず、非対応にもしない
                    if let Some(question) = custom_answer_question(name, string_schema) {
                        custom_answers.insert(question, name.clone());
                    }
                    continue;
                }
                let options = single_select_options(string_schema)?;
                (options, false, string_schema.title.clone())
            }
            v1::ElicitationPropertySchema::Array(array_schema) => {
                let options = multi_select_options(&array_schema.items)?;
                (options, true, array_schema.title.clone())
            }
            _ => return None, // 数値・真偽・未知の型が 1 つでもあれば非対応
        };
        if options.is_empty() {
            return None;
        }
        fields.push(ElicitationField {
            name: name.clone(),
            label: title.unwrap_or_else(|| name.clone()),
            options,
            multi,
            custom_answer: None,
        });
    }
    // Other 欄だけを読み飛ばした結果 0 件＝選ばせるものが無いフォーム。非対応として Decline に回す。
    if fields.is_empty() {
        return None;
    }
    for field in &mut fields {
        field.custom_answer = custom_answers.remove(&field.name);
    }
    Some(fields)
}

/// Other 欄がどの質問に付属するか。`_meta` の `questionId` を正とし、無ければ名前の
/// `<質問>_custom` 形から引く（Claude のブリッジは両方付ける・印だけのブリッジもあり得る）。
fn custom_answer_question(name: &str, schema: &v1::StringPropertySchema) -> Option<String> {
    let from_meta = schema
        .meta
        .as_ref()
        .and_then(|meta| meta.get(CUSTOM_ANSWER_META_KEY))
        .and_then(|marker| marker.get("questionId"))
        .and_then(|question| question.as_str());
    from_meta
        .or_else(|| name.strip_suffix("_custom"))
        .map(str::to_string)
}

/// 単一選択 string の選択肢（`oneOf` 優先・無ければ `enum`）。どちらも無い自由入力は `None`＝非対応。
fn single_select_options(schema: &v1::StringPropertySchema) -> Option<Vec<ElicitationChoice>> {
    if let Some(one_of) = &schema.one_of {
        return Some(one_of.iter().map(enum_option_choice).collect());
    }
    let enum_values = schema.enum_values.as_ref()?;
    Some(
        enum_values
            .iter()
            .map(|value| ElicitationChoice {
                value: value.clone(),
                title: value.clone(),
                description: None,
            })
            .collect(),
    )
}

/// 複数選択 array の選択肢（`items.anyOf` = タイトル付き / `items.enum` = 値のみ）。
/// 未知の items 形は `None`＝非対応（読めない選択肢を空カードで出さない）。
fn multi_select_options(items: &v1::MultiSelectItems) -> Option<Vec<ElicitationChoice>> {
    match items {
        v1::MultiSelectItems::Titled(titled) => {
            Some(titled.options.iter().map(enum_option_choice).collect())
        }
        v1::MultiSelectItems::String(strings) => Some(
            strings
                .values
                .iter()
                .map(|value| ElicitationChoice {
                    value: value.clone(),
                    title: value.clone(),
                    description: None,
                })
                .collect(),
        ),
        _ => None,
    }
}

fn enum_option_choice(option: &v1::EnumOption) -> ElicitationChoice {
    ElicitationChoice {
        value: option.value.clone(),
        title: option.title.clone(),
        description: option.description.clone(),
    }
}

/// 選択欄に付属する任意の「Other」自由入力欄か（`_meta` の共有印で判定）。
/// 選択肢を持つ string には印が付いていても選択欄として扱う（印だけで捨てない防御）。
fn is_custom_answer_field(schema: &v1::StringPropertySchema) -> bool {
    if schema.one_of.is_some() || schema.enum_values.is_some() {
        return false;
    }
    schema
        .meta
        .as_ref()
        .is_some_and(|meta| meta.contains_key(CUSTOM_ANSWER_META_KEY))
}

/// サブエージェントの手順の親（O17）: `_meta.claudeCode.parentToolUseId`。空文字は無いものとする。
fn subagent_parent(meta: Option<&v1::Meta>) -> Option<String> {
    meta?
        .get("claudeCode")?
        .get("parentToolUseId")?
        .as_str()
        .filter(|parent| !parent.is_empty())
        .map(str::to_string)
}

/// `session/request_elicitation`（選択肢付き質問）を UI へ橋渡しして応答する。form かつ全フィールドが
/// 単一選択の時だけ [`AgentEvent::ElicitationRequest`] を流し、UI の選択を Accept で返す。非対応な
/// フォーム（テキスト等を含む）は UI に出さず即 Decline する。UI が sender を drop したら Decline。
async fn handle_elicitation_request(
    request: v1::CreateElicitationRequest,
    responder: acp::Responder<v1::CreateElicitationResponse>,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), acp::Error> {
    let fields = match &request.mode {
        v1::ElicitationMode::Form(form) => simplify_elicitation_form(&form.requested_schema),
        _ => None,
    };
    let Some(fields) = fields else {
        return responder.respond(v1::CreateElicitationResponse::new(
            v1::ElicitationAction::Decline,
        ));
    };

    // 応答の型はフィールドの種類で決まる（単一選択 = string / 複数選択 = string の配列）。
    // fields はイベントへ渡して手放すので、名前 → 複数選択かの対応だけ先に控える。
    let multi_fields: std::collections::BTreeSet<String> = fields
        .iter()
        .filter(|field| field.multi)
        .map(|field| field.name.clone())
        .collect();

    let (respond_tx, mut respond_rx) = mpsc::unbounded::<Option<Vec<(String, Vec<String>)>>>();
    event_tx
        .unbounded_send(AgentEvent::ElicitationRequest {
            message: request.message.clone(),
            fields,
            respond: respond_tx,
        })
        .ok();

    // ユーザーの決定を待つ。Some(選択群)=Accept / None or drop=Decline。
    let action = match respond_rx.next().await.flatten() {
        Some(selections) => {
            let content: std::collections::BTreeMap<String, v1::ElicitationContentValue> =
                selections
                    .into_iter()
                    .map(|(name, values)| {
                        // 複数選択は 1 つしか選ばれていなくても配列で返す（スキーマの型に合わせる）。
                        let value = if multi_fields.contains(&name) {
                            v1::ElicitationContentValue::StringArray(values)
                        } else {
                            v1::ElicitationContentValue::from(
                                values.into_iter().next().unwrap_or_default(),
                            )
                        };
                        (name, value)
                    })
                    .collect();
            v1::ElicitationAction::Accept(v1::ElicitationAcceptAction::new().content(content))
        }
        None => v1::ElicitationAction::Decline,
    };
    responder.respond(v1::CreateElicitationResponse::new(action))
}

#[cfg(test)]
mod tests {
    /// 認証切れの判定: claude-agent-acp の実文言（OAuth 失効）と Codex/Copilot 系の定型句を拾い、
    /// API のストリーミング切断・タイムアウトなど**再ログインで直らない失敗**は拾わない。
    #[test]
    fn auth_failure_is_detected_from_agent_messages() {
        assert!(super::is_auth_failure(
            "Internal error: Failed to authenticate: OAuth session expired and could not be \
             refreshed: {\"errorKind\":\"authentication_failed\"}"
        ));
        assert!(super::is_auth_failure("Not logged in. Run `codex login`."));
        assert!(super::is_auth_failure("401 Unauthorized"));
        assert!(super::is_auth_failure(
            "Invalid API key · Please run /login"
        ));
        assert!(!super::is_auth_failure(
            "Internal error: API Error: Connection closed mid-response"
        ));
        assert!(!super::is_auth_failure(
            "ACP initialize が 30 秒応答しません（無言ハング）"
        ));
        assert!(!super::is_auth_failure("rate limit exceeded"));
    }

    /// セッション終了の判定: adapter の定型句（query ストリーム死亡・プロセス異常終了）だけを拾い、
    /// 認証切れやストリーミング切断など**同じセッションで再送できる失敗**は拾わない。
    #[test]
    fn session_ended_is_detected_from_adapter_messages() {
        assert!(super::is_session_ended(
            "Internal error: The Claude Agent session has ended. Please start a new session."
        ));
        assert!(super::is_session_ended(
            "Internal error: The Claude Agent process exited unexpectedly. Please start a new session."
        ));
        assert!(super::is_session_ended("Session not found"));
        assert!(!super::is_session_ended(
            "Internal error: Failed to authenticate: OAuth session expired and could not be refreshed"
        ));
        assert!(!super::is_session_ended(
            "Internal error: API Error: Connection closed mid-response"
        ));
    }

    /// 再ログインコマンドは実機検証済みの Claude / Codex に必ずある（案内文の空欄を防ぐ）。
    #[test]
    fn verified_agents_have_login_command() {
        let claude = super::AgentKind::by_label("Claude Code").expect("Claude Code");
        assert_eq!(claude.login_command(), Some("claude auth login"));
        let codex = super::AgentKind::by_label("Codex").expect("Codex");
        assert_eq!(codex.login_command(), Some("codex login"));
    }

    use super::*;

    #[test]
    fn claude_command_lookup_is_optional() {
        // PATH に無い環境でも None を返すだけ（パニックしない）
        let _ = AgentCommand::claude(".");
        assert!(find_in_path("definitely-not-a-real-binary-xyz").is_none());
    }

    /// レジストリの最小サンプル（claude だけ、組み込みより新しい版）。
    fn sample_registry() -> registry::Registry {
        registry::parse(
            r#"{"version":"1.0.0","agents":[
              {"id":"claude-acp","name":"Claude Code","version":"99.0.0","description":"",
               "distribution":{"npx":{"package":"@agentclientprotocol/claude-agent-acp@99.0.0",
                                      "env":{"FROM_REGISTRY":"1"}}}}
            ]}"#,
        )
        .expect("サンプルを読める")
    }

    fn claude() -> &'static AgentKind {
        AGENTS.iter().find(|a| a.id == "claude").expect("claude")
    }

    #[test]
    fn settings_command_wins_over_registry_and_catalog() {
        let settings = AgentOverride {
            command: Option::Some("definitely-not-a-real-agent-xyz".to_string()),
            args: vec!["--flag".to_string()],
            env: [("MY_KEY".to_string(), "v".to_string())]
                .into_iter()
                .collect(),
        };
        let resolved = claude()
            .resolve_command(".", Some(&settings), Some(&sample_registry()))
            .expect("設定があれば必ず解決する");
        // PATH に無い名前でもそのまま使う＝ユーザーの指定を勝手に捨てない。
        assert!(resolved.path.ends_with("definitely-not-a-real-agent-xyz"));
        assert_eq!(resolved.args.first().map(String::as_str), Some("--flag"));
        assert_eq!(resolved.env.get("MY_KEY").map(String::as_str), Some("v"));
        // レジストリの env は混ざらない（コマンドごと差し替えた＝レジストリは見ていない）。
        assert!(!resolved.env.contains_key("FROM_REGISTRY"));
    }

    #[test]
    fn registry_version_beats_the_builtin_catalog() {
        // npx が無い環境ではレジストリ経路に入れないので、その場合だけ skip する。
        if find_in_path("npx").is_none() {
            return;
        }
        let resolved = claude()
            .resolve_command(".", None, Some(&sample_registry()))
            .expect("レジストリから解決する");
        let spec = resolved.args.join(" ");
        // 組み込みは 0.73.0。レジストリの 99.0.0 が上限範囲として使われる。
        assert!(
            spec.contains("0.0.0 - 99.0.0"),
            "レジストリ版が使われていない: {spec}"
        );
        assert_eq!(
            resolved.env.get("FROM_REGISTRY").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn registry_env_only_override_keeps_registry_command() {
        if find_in_path("npx").is_none() {
            return;
        }
        // `type: "registry"` 相当＝command は無く env だけ。
        let settings = AgentOverride {
            command: None,
            args: Vec::new(),
            env: [("EXTRA".to_string(), "yes".to_string())]
                .into_iter()
                .collect(),
        };
        let resolved = claude()
            .resolve_command(".", Some(&settings), Some(&sample_registry()))
            .expect("解決する");
        assert!(resolved.args.join(" ").contains("0.0.0 - 99.0.0"));
        // レジストリの env と設定の env が両方載る。
        assert_eq!(
            resolved.env.get("FROM_REGISTRY").map(String::as_str),
            Some("1")
        );
        assert_eq!(resolved.env.get("EXTRA").map(String::as_str), Some("yes"));
    }

    #[test]
    fn falls_back_to_catalog_without_registry() {
        // レジストリが無くても（オフライン初回など）組み込みカタログで解決を試みる。
        // PATH 事情で None になり得るが、パニックしないことと版が組み込みであることを見る。
        if let Some(resolved) = claude().resolve_command(".", None, None) {
            let spec = resolved.args.join(" ");
            if spec.contains("claude-agent-acp") {
                assert!(
                    spec.contains("0.73.0"),
                    "組み込み版が使われていない: {spec}"
                );
            }
        }
    }

    #[test]
    fn npm_spec_becomes_bounded_range() {
        // scope 付き: 末尾の `@` だけが版の区切り（先頭の scope `@` は触らない）。
        assert_eq!(
            bounded_npm_spec("@agentclientprotocol/codex-acp@1.8.0"),
            "@agentclientprotocol/codex-acp@0.0.0 - 1.8.0"
        );
        // scope 無し。
        assert_eq!(bounded_npm_spec("cline@3.0.60"), "cline@0.0.0 - 3.0.60");
        // 版指定が無いものは常に最新のまま（`opencode-ai`）。
        assert_eq!(bounded_npm_spec("opencode-ai"), "opencode-ai");
        // scope だけで版が無い形も壊さない。
        assert_eq!(bounded_npm_spec("@scope/name"), "@scope/name");
        // タグ指定（数字で始まらない）は上限を二重に付けない。
        assert_eq!(bounded_npm_spec("pkg@latest"), "pkg@latest");
    }

    #[test]
    fn every_pinned_agent_package_survives_bounding() {
        // カタログの全 package が範囲化しても空にならない＝npx へ渡せる形であること。
        for agent in AGENTS {
            let Some(package) = agent.package else {
                continue;
            };
            let bounded = bounded_npm_spec(package);
            assert!(!bounded.is_empty(), "{}: 空の spec", agent.id);
            assert!(
                bounded.starts_with(package.split('@').next().unwrap_or(package))
                    || bounded.starts_with('@'),
                "{}: パッケージ名が壊れた: {bounded}",
                agent.id
            );
        }
    }

    #[test]
    fn elicitation_single_select_is_supported_others_declined() {
        use serde_json::json;
        // oneOf 付き string（4 択質問）= 対応。ラベル・選択肢が正しく簡約される。
        let schema: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "answer": {
                    "type": "string",
                    "title": "方針",
                    "oneOf": [
                        {"const": "a", "title": "案A", "description": "説明A"},
                        {"const": "b", "title": "案B"}
                    ]
                }
            }
        }))
        .expect("schema をパースできる");
        let fields = simplify_elicitation_form(&schema).expect("単一選択フォームは対応");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "answer");
        assert_eq!(fields[0].label, "方針");
        assert_eq!(fields[0].options.len(), 2);
        assert_eq!(fields[0].options[0].value, "a");
        assert_eq!(fields[0].options[0].title, "案A");
        assert_eq!(fields[0].options[0].description.as_deref(), Some("説明A"));

        // enum（タイトル無し単一選択）も対応＝値がそのまま表示名になる。
        let enum_schema: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": { "size": {"type": "string", "enum": ["S", "M", "L"]} }
        }))
        .expect("schema をパースできる");
        let enum_fields = simplify_elicitation_form(&enum_schema).expect("enum も対応");
        assert_eq!(enum_fields[0].options.len(), 3);
        assert_eq!(enum_fields[0].options[1].title, "M");

        // boolean を含む＝この UI では正しく返せないので非対応（呼び出し側が Decline）。
        let bool_schema: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": { "ok": {"type": "boolean"} }
        }))
        .expect("schema をパースできる");
        assert!(simplify_elicitation_form(&bool_schema).is_none());

        // 選択肢の無い自由入力 string も非対応（テキスト入力になるため）。
        let text_schema: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": { "note": {"type": "string"} }
        }))
        .expect("schema をパースできる");
        assert!(simplify_elicitation_form(&text_schema).is_none());
    }

    /// Claude Code の AskUserQuestion がそのまま届く形。質問ごとに「選択欄 + 任意の Other 欄」の
    /// 2 プロパティが来るので、Other 欄で非対応にすると質問が UI に出ない（実際に出なかった）。
    #[test]
    fn ask_user_question_custom_answer_field_is_skipped_not_rejected() {
        use serde_json::json;
        let schema: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "question_0": {
                    "type": "string",
                    "title": "PDF の見せ方",
                    "oneOf": [
                        {"const": "A: アプリ内タブ", "title": "A: アプリ内タブ", "description": "OS の WebView に載せる"},
                        {"const": "B: 外部アプリ", "title": "B: 外部アプリ"}
                    ]
                },
                "question_0_custom": {
                    "type": "string",
                    "title": "Other",
                    "description": "Type your own answer instead of choosing an option above (optional).",
                    "_meta": {
                        "_askUserQuestionCustomAnswer": {
                            "questionId": "question_0",
                            "isCustomAnswer": true
                        }
                    }
                }
            }
        }))
        .expect("schema をパースできる");
        let fields = simplify_elicitation_form(&schema).expect("Other 欄つきでも質問は出せる");
        assert_eq!(fields.len(), 1, "Other 欄は選択欄として数えない");
        assert_eq!(fields[0].name, "question_0");
        assert_eq!(fields[0].label, "PDF の見せ方");
        assert_eq!(fields[0].options.len(), 2);
        assert_eq!(
            fields[0].custom_answer.as_deref(),
            Some("question_0_custom"),
            "Other 欄は付属先の質問に結ぶ（O17・自由入力欄を出す）"
        );

        // 印が無いただの自由入力なら従来どおり非対応（返せない入力を勝手に握り潰さない）。
        let unmarked: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "question_0": {"type": "string", "enum": ["a", "b"]},
                "free": {"type": "string", "title": "自由記述"}
            }
        }))
        .expect("schema をパースできる");
        assert!(simplify_elicitation_form(&unmarked).is_none());

        // multiSelect の質問（`type: array` + `items.anyOf`）も対応する。
        // ここを弾いていた間は「複数選んでください」系の質問がまるごと UI に出なかった。
        let multi: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "question_0": {
                    "type": "array",
                    "title": "入れる機能",
                    "items": {"anyOf": [
                        {"const": "検索", "title": "検索", "description": "全文検索"},
                        {"const": "Git", "title": "Git"}
                    ]}
                },
                "question_0_custom": {
                    "type": "string",
                    "_meta": {"_askUserQuestionCustomAnswer": {"isCustomAnswer": true}}
                }
            }
        }))
        .expect("schema をパースできる");
        let multi_fields = simplify_elicitation_form(&multi).expect("複数選択も対応");
        assert_eq!(multi_fields.len(), 1);
        assert!(multi_fields[0].multi, "複数選択として印を付ける");
        assert_eq!(
            multi_fields[0].custom_answer.as_deref(),
            Some("question_0_custom"),
            "questionId が無ければ名前の <質問>_custom 形で結ぶ"
        );
        assert_eq!(multi_fields[0].label, "入れる機能");
        assert_eq!(multi_fields[0].options.len(), 2);
        assert_eq!(
            multi_fields[0].options[0].description.as_deref(),
            Some("全文検索")
        );

        // タイトル無しの複数選択（items.enum）も値がそのまま表示名になる。
        let plain_multi: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "tags": {"type": "array", "items": {"type": "string", "enum": ["a", "b", "c"]}}
            }
        }))
        .expect("schema をパースできる");
        let plain = simplify_elicitation_form(&plain_multi).expect("items.enum も対応");
        assert!(plain[0].multi);
        assert_eq!(plain[0].options[2].title, "c");
        assert!(
            plain[0].custom_answer.is_none(),
            "Other 欄の無い質問は入力欄を出さない"
        );

        // Other 欄しか無い＝選ばせるものが無いので非対応。
        let only_custom: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "question_0_custom": {
                    "type": "string",
                    "_meta": {"_askUserQuestionCustomAnswer": {"isCustomAnswer": true}}
                }
            }
        }))
        .expect("schema をパースできる");
        assert!(simplify_elicitation_form(&only_custom).is_none());
    }

    /// Other 欄の結び先（O17）: `questionId` を名前より優先し、名前順で選択欄より先に来ても結ぶ。
    /// 付属先の無い Other 欄は捨てる（任意入力なので、読み飛ばしても答えは作れる）。
    #[test]
    fn custom_answer_fields_attach_to_their_question() {
        use serde_json::json;
        let schema: v1::ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "a_note": {
                    "type": "string",
                    "_meta": {"_askUserQuestionCustomAnswer": {"questionId": "z_pick"}}
                },
                "z_pick": {"type": "string", "enum": ["x", "y"]},
                "other_custom": {
                    "type": "string",
                    "_meta": {"_askUserQuestionCustomAnswer": {"isCustomAnswer": true}}
                }
            }
        }))
        .expect("schema をパースできる");
        let fields = simplify_elicitation_form(&schema).expect("選択欄が 1 つある");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "z_pick");
        assert_eq!(fields[0].custom_answer.as_deref(), Some("a_note"));
    }

    /// 回帰テスト: `session/prompt` がエラー応答（例: API のストリーミング切断「Connection
    /// closed mid-response」）で終わっても、セッションは死なず**そのターンだけ** Failed で畳まれ、
    /// 同じセッションで次の prompt が完走すること。かつては `Session::send_prompt` が応答ハンドラを
    /// 接続内タスクに spawn していたため、エラーが接続ドライバごと落として「ACP セッションが
    /// 異常終了」になっていた。偽エージェント（python3 の改行区切り JSON-RPC）で再現する。
    #[test]
    fn prompt_error_fails_turn_but_keeps_session() {
        const FAKE_AGENT: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

prompts = 0
for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1)}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "sess-1"}})
    elif method == "session/prompt":
        prompts += 1
        if prompts == 1:
            send({"jsonrpc": "2.0", "id": rid,
                  "error": {"code": -32603,
                            "message": "Internal error: API Error: Connection closed mid-response"}})
        else:
            send({"jsonrpc": "2.0", "method": "session/update",
                  "params": {"sessionId": "sess-1",
                             "update": {"sessionUpdate": "agent_message_chunk",
                                        "content": {"type": "text", "text": "recovered"}}}})
            send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
"#;
        let Some(python) = find_in_path("python3") else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let cwd = std::env::current_dir().expect("cwd");
        let command = AgentCommand::new(python, vec!["-c".into(), FAKE_AGENT.into()], cwd);
        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, mut event_rx) = mpsc::unbounded();
        command_tx
            .unbounded_send(SessionCommand::Prompt("1回目".into()))
            .expect("send");

        let events = futures::executor::block_on(async move {
            let session = run_session(command, SessionPreferences::default(), command_rx, event_tx);
            let scenario = async move {
                let mut seen = Vec::new();
                // 1 ターン目: エラー応答 → Failed（終端）。ここでセッションはまだ生きている。
                loop {
                    let event = event_rx.next().await.expect("イベントが途切れた");
                    let failed = matches!(event, AgentEvent::Failed(_));
                    seen.push(event);
                    if failed {
                        break;
                    }
                }
                // 同じセッションへ再送 → 今度は完走する。command_tx はターン完走**後**に
                // drop する（ターン中に閉じるとループが畳まれて途中終了してしまう）。
                command_tx
                    .unbounded_send(SessionCommand::Prompt("2回目".into()))
                    .expect("再送 send");
                loop {
                    let event = event_rx.next().await.expect("イベントが途切れた");
                    let ended = matches!(event, AgentEvent::TurnEnded { .. });
                    seen.push(event);
                    if ended {
                        break;
                    }
                }
                drop(command_tx);
                seen
            };
            let (outcome, seen) = futures::join!(session, scenario);
            outcome.expect("セッションは正常終了する");
            seen
        });

        // 先頭はセッション開始（新規・id は偽エージェントの "sess-1"）。
        match &events[0] {
            AgentEvent::SessionStarted {
                session_id,
                resumed,
                ..
            } => {
                assert_eq!(session_id, "sess-1");
                assert!(!resumed);
            }
            other => panic!("SessionStarted を期待: {other:?}"),
        }
        assert!(matches!(events[1], AgentEvent::TurnStarted), "{events:?}");
        match &events[2] {
            AgentEvent::Failed(message) => assert!(
                message.contains("Connection closed mid-response"),
                "{message}"
            ),
            other => panic!("Failed を期待: {other:?}"),
        }
        assert!(matches!(events[3], AgentEvent::TurnStarted), "{events:?}");
        match &events[4] {
            AgentEvent::AgentChunk(text) => assert_eq!(text, "recovered"),
            other => panic!("AgentChunk を期待: {other:?}"),
        }
        assert!(
            matches!(
                events[5],
                AgentEvent::TurnEnded {
                    reason: TurnEnd::Completed
                }
            ),
            "{events:?}"
        );
        assert_eq!(events.len(), 6, "{events:?}");
    }

    /// 偽エージェントを走らせ、イベントチャネルが閉じるまで（＝セッションが終わるまで）の
    /// 全イベントと `run_session` の結果を返す。transport 断系のテストの共通土台。
    /// 偽エージェントの stdio を **UTF-8 に固定する**前置き。
    ///
    /// Python は Windows で stdin/stdout を**ロケールの符号**（US の runner なら cp1252）で読み書き
    /// する。ACP の線を流れる日本語は UTF-8 なので、そのままだと復号できないバイト
    /// （`こ` = E3 81 93 の `0x81` など）が `surrogateescape` で孤立サロゲートに化け、エージェントが
    /// 返す JSON をこちらが読めなくなる（Windows CI の `lone leading surrogate in hex escape`・
    /// 2026-09-19）。**mac では再現しない**ので、偽エージェントを足すときは必ずここを通すこと。
    const FAKE_AGENT_UTF8: &str = "import sys\nsys.stdin.reconfigure(encoding='utf-8')\n\
sys.stdout.reconfigure(encoding='utf-8')\n";

    fn fake_agent_command(script: &str) -> Option<AgentCommand> {
        let python = find_in_path("python3")?;
        let cwd = std::env::current_dir().expect("cwd");
        let script = format!("{FAKE_AGENT_UTF8}{script}");
        Some(AgentCommand::new(python, vec!["-c".into(), script], cwd))
    }

    fn run_fake_agent_until_session_ends(
        script: &str,
        preferences: SessionPreferences,
        first_prompt: Option<&str>,
    ) -> Option<(Result<()>, Vec<AgentEvent>)> {
        let command = fake_agent_command(script)?;
        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, mut event_rx) = mpsc::unbounded();
        if let Some(prompt) = first_prompt {
            command_tx
                .unbounded_send(SessionCommand::Prompt(prompt.into()))
                .expect("send");
        }
        Some(futures::executor::block_on(async move {
            let session = run_session(command, preferences, command_rx, event_tx);
            let collect = async move {
                let mut seen = Vec::new();
                // 送信ハンドルは握ったまま。セッションが**自分から**終わることを検証する。
                while let Some(event) = event_rx.next().await {
                    seen.push(event);
                }
                drop(command_tx);
                seen
            };
            futures::join!(session, collect)
        }))
    }

    /// 偽エージェント: `session/prompt` を受けた瞬間にプロセスごと消える（SSH 切断で remote の
    /// エージェントが SIGHUP で死ぬのと同じ見え方＝ stdout が EOF になる）。
    const AGENT_THAT_DIES_ON_PROMPT: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1)}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "sess-1"}})
    elif method == "session/prompt":
        sys.exit(0)
"#;

    /// 回帰テスト（2026-09-08）: ターン中に transport が閉じたら、そのターンだけでなく
    /// **セッションごと**終わる。以前は「そのターンだけ Failed」でセッションを維持していたため、
    /// 送るたびに「Incoming transport closed」が即返るゾンビになっていた。
    #[test]
    fn transport_close_during_prompt_ends_session() {
        let Some((outcome, events)) = run_fake_agent_until_session_ends(
            AGENT_THAT_DIES_ON_PROMPT,
            SessionPreferences::default(),
            Some("1回目"),
        ) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        outcome.expect("transport 断は異常終了ではなく、畳んで正常に戻る");
        assert!(
            matches!(events[0], AgentEvent::SessionStarted { .. }),
            "{events:?}"
        );
        assert!(matches!(events[1], AgentEvent::TurnStarted), "{events:?}");
        assert!(
            matches!(events.last(), Some(AgentEvent::SessionLost)),
            "最後は SessionLost（Failed ではない）: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::Failed(_))),
            "transport 断を Failed として二重に報告しない: {events:?}"
        );
    }

    /// 偽エージェント: `session/new` に答えた直後に消える（待機中の切断）。
    const AGENT_THAT_DIES_WHILE_IDLE: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1)}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "sess-1"}})
        sys.exit(0)
"#;

    /// 待機中（prompt を送っていない）に transport が閉じても、次の送信を待たずに SessionLost が
    /// 届いてセッションが終わる。UI はこれで送信前に「切れている」と表示できる。
    #[test]
    fn transport_close_while_idle_ends_session() {
        let Some((outcome, events)) = run_fake_agent_until_session_ends(
            AGENT_THAT_DIES_WHILE_IDLE,
            SessionPreferences::default(),
            None,
        ) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        outcome.expect("待機中の transport 断も正常に畳む");
        assert!(
            matches!(events[0], AgentEvent::SessionStarted { .. }),
            "{events:?}"
        );
        assert!(
            matches!(events.last(), Some(AgentEvent::SessionLost)),
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::TurnStarted)),
            "prompt を送っていないのでターンは始まらない: {events:?}"
        );
    }

    /// 偽エージェント（`LOAD_CAP` を true/false に置換して使う）: `loadSession` の広告有無で
    /// `session/load` が使われるかが変わる。load 時は履歴の再生（古いチャンク）を流してから応答する。
    /// prompt 応答には受けた method の順を埋め込む＝どちらの経路を通ったかを本文で検証できる。
    const AGENT_WITH_LOAD_SESSION: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

seen = []
for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    seen.append(method)
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1),
                         "agentCapabilities": {"loadSession": LOAD_CAP}}})
    elif method == "session/load":
        sid = params["sessionId"]
        send({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": sid,
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": "old history"}}}})
        send({"jsonrpc": "2.0", "id": rid, "result": {}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "fresh"}})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": params["sessionId"],
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text",
                                                "text": "order=" + ",".join(seen)}}}})
        send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
        sys.exit(0)
"#;

    fn resume_scenario(load_advertised: bool) -> Option<Vec<AgentEvent>> {
        // python の真偽値は大文字（json.dumps が JSON の true/false へ直す）。
        let script = AGENT_WITH_LOAD_SESSION
            .replace("LOAD_CAP", if load_advertised { "True" } else { "False" });
        let preferences = SessionPreferences {
            resume: Some("prev-1".into()),
            ..SessionPreferences::default()
        };
        let (outcome, events) =
            run_fake_agent_until_session_ends(&script, preferences, Some("続き"))?;
        outcome.expect("セッションは正常終了する");
        Some(events)
    }

    /// `loadSession` を広告するエージェントには `session/load` で前回の id を渡し、同じ会話を
    /// 引き継ぐ。load 中に再生される履歴は transcript に流さない（二重表示しない）。
    #[test]
    fn resume_loads_previous_session_and_skips_replayed_history() {
        let Some(events) = resume_scenario(true) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        match &events[0] {
            AgentEvent::SessionStarted {
                session_id,
                resumed,
                ..
            } => {
                assert_eq!(session_id, "prev-1");
                assert!(resumed, "session/load で引き継いだ");
            }
            other => panic!("SessionStarted を期待: {other:?}"),
        }
        let chunks: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::AgentChunk(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            chunks,
            vec!["order=initialize,session/load,session/prompt"],
            "再生された履歴（old history）は流れず、session/new も呼ばれない: {events:?}"
        );
    }

    /// 広告が無ければ前回 id があっても `session/new`（引き継がず・失敗もしない）。
    #[test]
    fn resume_falls_back_to_new_session_when_load_is_not_advertised() {
        let Some(events) = resume_scenario(false) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        match &events[0] {
            AgentEvent::SessionStarted {
                session_id,
                resumed,
                ..
            } => {
                assert_eq!(session_id, "fresh");
                assert!(!resumed);
            }
            other => panic!("SessionStarted を期待: {other:?}"),
        }
        let chunks: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::AgentChunk(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            chunks,
            vec!["order=initialize,session/new,session/prompt"],
            "{events:?}"
        );
    }

    /// 偽エージェント（`LOAD_CAP` を置換して使う）: `session/new` / `session/load` で受け取った
    /// `_meta` を、どちらの method で受けたかと一緒に prompt 応答へ載せる。キーが無ければ `null`。
    const AGENT_THAT_ECHOES_META: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

received = {}
for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1),
                         "agentCapabilities": {"loadSession": LOAD_CAP}}})
    elif method in ("session/new", "session/load"):
        received = {"method": method, "has_meta": "_meta" in params, "meta": params.get("_meta")}
        result = {} if method == "session/load" else {"sessionId": "fresh"}
        send({"jsonrpc": "2.0", "id": rid, "result": result})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": params["sessionId"],
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text",
                                                "text": json.dumps(received, sort_keys=True)}}}})
        send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
        sys.exit(0)
"#;

    fn echoed_session_params(
        preferences: SessionPreferences,
        load_advertised: bool,
    ) -> Option<serde_json::Value> {
        let script = AGENT_THAT_ECHOES_META
            .replace("LOAD_CAP", if load_advertised { "True" } else { "False" });
        let (outcome, events) =
            run_fake_agent_until_session_ends(&script, preferences, Some("やあ"))?;
        outcome.expect("セッションは正常終了する");
        let echoed = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::AgentChunk(text) => Some(text.as_str()),
                _ => None,
            })
            .expect("エージェントが受け取った params が返る");
        Some(serde_json::from_str(echoed).expect("JSON で返る"))
    }

    fn chat_like_preset() -> preset::SessionPreset {
        preset::SessionPreset {
            system_prompt: Some("質問には会話で答える".to_string()),
            tools: Some(vec!["Read".to_string(), "Write".to_string()]),
            setting_sources: Some(Vec::new()),
            ..preset::SessionPreset::default()
        }
    }

    /// プリセットの `_meta` は **`session/new` と `session/load` の両方**に同じ形で乗る。
    /// load 側に載せ忘れると、再起動して会話を再開した瞬間に Chat がコーディングエージェントへ戻る
    /// （アダプタは `loadSession` でも `params._meta` からセッションを組み立てる）。
    #[test]
    fn the_preset_meta_rides_on_both_new_and_load() {
        let expected = serde_json::Value::Object(chat_like_preset().to_meta().expect("meta"));

        let fresh = SessionPreferences {
            preset: chat_like_preset(),
            ..SessionPreferences::default()
        };
        let Some(received) = echoed_session_params(fresh, true) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        assert_eq!(received["method"], "session/new", "{received}");
        assert_eq!(received["meta"], expected, "{received}");

        let resumed = SessionPreferences {
            preset: chat_like_preset(),
            resume: Some("prev-1".into()),
            ..SessionPreferences::default()
        };
        let received = echoed_session_params(resumed, true).expect("python3 は上で確認済み");
        assert_eq!(received["method"], "session/load", "{received}");
        assert_eq!(received["meta"], expected, "{received}");
    }

    /// プリセットが空なら `_meta` キーごと送らない（`null` も送らない）＝既存のスレッドは線の上で不変。
    #[test]
    fn no_preset_means_no_meta_key_on_the_wire() {
        let Some(received) = echoed_session_params(SessionPreferences::default(), false) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        assert_eq!(received["method"], "session/new", "{received}");
        assert_eq!(received["has_meta"], false, "{received}");
    }

    /// npx のキャッシュに**指定と同じ版**が在る時だけ、そこを直接起こす。
    #[cfg(not(windows))]
    #[test]
    fn a_cached_adapter_of_the_exact_version_is_launched_directly() {
        let cache = std::env::temp_dir().join(format!(
            "necoder_npx_cache_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let install = |hash: &str, version: &str| {
            let modules = cache.join(hash).join("node_modules");
            let package = modules.join("@scope/adapter");
            std::fs::create_dir_all(&package).unwrap();
            std::fs::create_dir_all(modules.join(".bin")).unwrap();
            std::fs::write(
                package.join("package.json"),
                format!(r#"{{"name":"@scope/adapter","version":"{version}"}}"#),
            )
            .unwrap();
            std::fs::write(modules.join(".bin/adapter"), "#!/usr/bin/env node\n").unwrap();
            modules.join(".bin/adapter")
        };
        install("aaa", "0.77.0");
        let wanted = install("bbb", "0.78.0");

        assert_eq!(
            npx_cached_agent(&cache, "@scope/adapter@0.78.0", "adapter"),
            Some(wanted)
        );
        // 無い版は npx に任せる（古いキャッシュを使い続けて更新が止まらないように）。
        assert_eq!(
            npx_cached_agent(&cache, "@scope/adapter@0.79.0", "adapter"),
            None
        );
        // 版が決まらない指定（範囲・タグ・版なし）も npx に任せる。
        assert_eq!(
            npx_cached_agent(&cache, "@scope/adapter@latest", "adapter"),
            None
        );
        assert_eq!(npx_cached_agent(&cache, "@scope/adapter", "adapter"), None);
        assert_eq!(
            npx_cached_agent(&cache.join("missing"), "@scope/adapter@0.78.0", "adapter"),
            None
        );
        let _ = std::fs::remove_dir_all(cache);
    }

    /// 偽エージェント（`IMAGE_CAP` を置換して使う）: 受け取った prompt のブロック列をそのまま返す。
    const AGENT_THAT_ECHOES_PROMPT_BLOCKS: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1),
                         "agentCapabilities": {"promptCapabilities": {"image": IMAGE_CAP}}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "img-1"}})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": params["sessionId"],
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text",
                                                "text": json.dumps(params.get("prompt"))}}}})
        send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
        sys.exit(0)
"#;

    fn image_prompt_scenario(agent_accepts_images: bool) -> Option<Vec<AgentEvent>> {
        let script = AGENT_THAT_ECHOES_PROMPT_BLOCKS.replace(
            "IMAGE_CAP",
            if agent_accepts_images {
                "True"
            } else {
                "False"
            },
        );
        let command = fake_agent_command(&script)?;
        let (command_tx, command_rx) = mpsc::unbounded::<SessionCommand>();
        let (event_tx, event_rx) = mpsc::unbounded::<AgentEvent>();
        command_tx
            .unbounded_send(SessionCommand::PromptWithImages {
                text: "このエラーは何？".into(),
                images: vec![PromptImage {
                    mime_type: "image/png".into(),
                    data: "aGVsbG8=".into(),
                }],
            })
            .ok()?;
        let (outcome, events) = futures::executor::block_on(futures::future::join(
            run_session(command, SessionPreferences::default(), command_rx, event_tx),
            event_rx.collect::<Vec<_>>(),
        ));
        drop(command_tx);
        outcome.expect("セッションは正常終了する");
        Some(events)
    }

    fn echoed_blocks(events: &[AgentEvent]) -> serde_json::Value {
        let echoed = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::AgentChunk(text) => Some(text.as_str()),
                _ => None,
            })
            .expect("エージェントが受け取った prompt が返る");
        serde_json::from_str(echoed).expect("JSON で返る")
    }

    /// 画像は ACP の image ブロックとして、**本文の前**に乗る。
    #[test]
    fn images_ride_as_image_blocks_before_the_text() {
        let Some(events) = image_prompt_scenario(true) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let blocks = echoed_blocks(&events);
        assert_eq!(blocks.as_array().map(Vec::len), Some(2), "{blocks}");
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["mimeType"], "image/png");
        assert_eq!(blocks[0]["data"], "aGVsbG8=");
        assert_eq!(blocks[1]["type"], "text");
        assert_eq!(blocks[1]["text"], "このエラーは何？");
    }

    /// 画像を受け取れないエージェントには送らない。黙って落とさず、1 行で知らせる。
    #[test]
    fn images_are_dropped_with_a_notice_when_the_agent_cannot_take_them() {
        let Some(events) = image_prompt_scenario(false) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let blocks = echoed_blocks(&events);
        assert_eq!(blocks.as_array().map(Vec::len), Some(1), "{blocks}");
        assert_eq!(blocks[0]["type"], "text");
        assert!(
            events.iter().any(
                |event| matches!(event, AgentEvent::Notice(message) if message.contains("画像"))
            ),
            "{events:?}"
        );
    }

    /// 偽エージェント: `session/new` で受け取った `mcpServers` をそのまま prompt 応答に載せる。
    /// **ACP の線に何が乗ったか**を本文で検証できる（渡し忘れると空配列が返って落ちる）。
    /// `mcpCapabilities` は http だけ広告する＝sse は渡してはいけない側の検証にも使う。
    const AGENT_THAT_ECHOES_MCP: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

received = []
for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1),
                         "agentCapabilities": {"mcpCapabilities": {"http": True, "sse": False}}}})
    elif method == "session/new":
        received = params.get("mcpServers") or []
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "mcp-1"}})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": params["sessionId"],
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text",
                                                "text": json.dumps(received, sort_keys=True)}}}})
        send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
        sys.exit(0)
"#;

    /// `session/new` に MCP サーバが実際に乗ること、広告の無い伝送方式は乗らずに知らせが出ること。
    /// **この層が無いと「Codex CLI に登録したのにスレッドから使えない」に戻る**（2026-09-10）。
    #[test]
    fn new_session_carries_the_enabled_mcp_servers_over_the_wire() {
        let server = |name: &str, transport| mcp::McpServerConfig {
            name: name.to_string(),
            source: mcp::McpSource::Necoder,
            enabled: true,
            transport,
        };
        let mut disabled = server(
            "muted",
            mcp::McpTransport::Stdio {
                command: "/usr/local/bin/muted".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        );
        disabled.enabled = false;
        let preferences = SessionPreferences {
            mcp_servers: vec![
                server(
                    "tools",
                    mcp::McpTransport::Stdio {
                        command: "/usr/local/bin/tools".into(),
                        args: vec!["--flag".into()],
                        env: BTreeMap::new(),
                    },
                ),
                server(
                    "higgsfield",
                    mcp::McpTransport::Http {
                        url: "https://mcp.example.invalid/mcp".into(),
                        headers: BTreeMap::new(),
                    },
                ),
                server(
                    "streamed",
                    mcp::McpTransport::Sse {
                        url: "https://mcp.example.invalid/sse".into(),
                        headers: BTreeMap::new(),
                    },
                ),
                disabled,
            ],
            ..SessionPreferences::default()
        };
        let Some((outcome, events)) =
            run_fake_agent_until_session_ends(AGENT_THAT_ECHOES_MCP, preferences, Some("やって"))
        else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        outcome.expect("セッションは正常終了する");

        let echoed = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::AgentChunk(text) => Some(text.as_str()),
                _ => None,
            })
            .expect("エージェントが受け取った mcpServers が返る");
        let received: serde_json::Value = serde_json::from_str(echoed).expect("JSON で返る");
        let names: Vec<&str> = received
            .as_array()
            .expect("配列")
            .iter()
            .filter_map(|server| server.get("name")?.as_str())
            .collect();
        // 有効な stdio と http だけが乗る。無効な行と、広告の無い sse は乗らない。
        assert_eq!(names, vec!["tools", "higgsfield"], "{echoed}");
        assert_eq!(received[0]["command"], "/usr/local/bin/tools");
        assert_eq!(received[0]["args"][0], "--flag");
        assert_eq!(received[1]["url"], "https://mcp.example.invalid/mcp");

        // 渡せなかったものは黙って落とさない（transcript に 1 行出す）。
        let notices: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Notice(message) => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("streamed"), "{}", notices[0]);
        assert!(
            !notices[0].contains("muted"),
            "無効な行は知らせにも出さない: {}",
            notices[0]
        );
    }

    #[test]
    fn config_choice_matches_name_or_id_case_insensitively() {
        let configs = vec![ConfigOption {
            config_id: "model".into(),
            category: ConfigCategory::Model,
            current: "claude-fable-5".into(),
            choices: vec![
                ("claude-opus-5".into(), "Opus".into()),
                ("claude-fable-5".into(), "Fable".into()),
            ],
        }];
        let by_name = find_config_choice(&configs, ConfigCategory::Model, "opus")
            .expect("表示名（大文字小文字無視）で引ける");
        assert_eq!(by_name.config_id, "model");
        assert_eq!(by_name.value_id, "claude-opus-5");
        assert!(!by_name.is_current, "current と違うので送る対象");
        let by_id = find_config_choice(&configs, ConfigCategory::Model, "CLAUDE-FABLE-5")
            .expect("value_id でも引ける");
        assert!(by_id.is_current, "current と同じなら送らない");
        assert!(
            find_config_choice(&configs, ConfigCategory::Model, "claude-sonnet-5").is_none(),
            "広告に無い希望は None（エージェント既定のまま）"
        );
        assert!(
            find_config_choice(&configs, ConfigCategory::ThoughtLevel, "Opus").is_none(),
            "カテゴリ違いは None"
        );
    }

    /// 遅延起動の初回ターン: UI からの `SetConfig` は Prompt の後ろに並ぶ（＝ターン中 deferred）ので、
    /// 起動時の `SessionPreferences` を**最初の prompt より前に**適用する。偽エージェントは受けた
    /// method を順に記録し、prompt 応答で「その時点の current」を返す＝初回ターンが希望どおりに
    /// 走ったかと、`set_config_option` が `session/prompt` より前に届いたかを同時に検証できる。
    #[test]
    fn preferences_apply_before_first_prompt() {
        const FAKE_AGENT: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

current = {"model": "claude-fable-5", "effort": "high"}
seen = []

def config_options():
    return [
        {"id": "model", "name": "Model", "category": "model", "type": "select",
         "currentValue": current["model"],
         "options": [{"value": "claude-opus-5", "name": "Opus"},
                     {"value": "claude-fable-5", "name": "Fable"}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
         "currentValue": current["effort"],
         "options": [{"value": "low", "name": "Low"},
                     {"value": "high", "name": "High"},
                     {"value": "max", "name": "Max"}]},
    ]

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    seen.append(method)
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1)}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"sessionId": "sess-1", "configOptions": config_options()}})
    elif method == "session/set_config_option":
        current[params["configId"]] = params["value"]
        send({"jsonrpc": "2.0", "id": rid, "result": {"configOptions": config_options()}})
    elif method == "session/prompt":
        text = "model=%s effort=%s order=%s" % (
            current["model"], current["effort"], ",".join(seen))
        send({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": "sess-1",
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": text}}}})
        send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
"#;
        let Some(python) = find_in_path("python3") else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let cwd = std::env::current_dir().expect("cwd");
        let command = AgentCommand::new(python, vec!["-c".into(), FAKE_AGENT.into()], cwd);
        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, mut event_rx) = mpsc::unbounded();
        // UI と同じ順序: セッション起動と同時に Prompt をキューへ積む（SetConfig はまだ送れない）。
        command_tx
            .unbounded_send(SessionCommand::Prompt("初回".into()))
            .expect("send");
        // 表示名（Opus）と value_id（max）の両方で引けること・大文字小文字を無視することを兼ねる。
        let preferences = SessionPreferences {
            mode: None,
            model: Some("opus".into()),
            effort: Some("MAX".into()),
            resume: None,
            mcp_servers: Vec::new(),
            preset: preset::SessionPreset::default(),
        };

        let events = futures::executor::block_on(async move {
            let session = run_session(command, preferences, command_rx, event_tx);
            let scenario = async move {
                let mut seen = Vec::new();
                loop {
                    let event = event_rx.next().await.expect("イベントが途切れた");
                    let ended = matches!(event, AgentEvent::TurnEnded { .. });
                    seen.push(event);
                    if ended {
                        break;
                    }
                }
                // command_tx はターン完走**後**に drop する（ターン中に閉じるとループが畳まれる）。
                drop(command_tx);
                seen
            };
            let (outcome, seen) = futures::join!(session, scenario);
            outcome.expect("セッションは正常終了する");
            seen
        });

        // UI へ届く一覧は**適用後**の current（UI 側の照合が一致し、二重送信にならない）。
        let configs = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::Configs(configs) => Some(configs),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Configs を期待: {events:?}"));
        let current_of = |category: ConfigCategory| {
            configs
                .iter()
                .find(|config| config.category == category)
                .map(|config| config.current.clone())
        };
        assert_eq!(
            current_of(ConfigCategory::Model).as_deref(),
            Some("claude-opus-5")
        );
        assert_eq!(
            current_of(ConfigCategory::ThoughtLevel).as_deref(),
            Some("max")
        );
        // 初回ターンは希望モデル/思考量で走り、set_config_option は prompt より前に届いている。
        let chunk = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::AgentChunk(text) => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("AgentChunk を期待: {events:?}"));
        assert_eq!(
            chunk,
            "model=claude-opus-5 effort=max order=initialize,session/new,\
             session/set_config_option,session/set_config_option,session/prompt"
        );
        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::TurnEnded {
                    reason: TurnEnd::Completed
                })
            ),
            "{events:?}"
        );
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
        let command = AgentCommand::new(
            find_in_path("sleep").expect("sleep が PATH に無い"),
            vec!["120".to_string()],
            std::env::temp_dir(),
        );
        let started = std::time::Instant::now();
        let result = futures::executor::block_on(connect_and_initialize(&command));
        let elapsed = started.elapsed();
        let error = result.expect_err("無言ハングは timeout エラーになる（無限待ちにならない）");
        assert!(
            format!("{error:#}").contains("応答しません"),
            "理由が UI に出る: {error:#}"
        );
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
        let states = futures::executor::block_on(refresh_agent_auth_states(cwd, &[]));
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
            let session = run_session(command, SessionPreferences::default(), prompt_rx, event_tx);
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
                        AgentEvent::SessionCost { amount, currency } => {
                            eprintln!("[cost] {amount} {currency}")
                        }
                        AgentEvent::RateLimits(limits) => eprintln!("[rate limits] {limits:?}"),
                        AgentEvent::TurnUsage(tokens) => eprintln!("[turn usage] {tokens:?}"),
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
                        AgentEvent::Commands(commands) => {
                            eprintln!("[commands] {} items", commands.len())
                        }
                        AgentEvent::TitleChanged(title) => eprintln!("[title] {title:?}"),
                        AgentEvent::GoalChanged(goal) => eprintln!("[goal] {goal:?}"),
                        AgentEvent::GoalControls(actions) => {
                            eprintln!("[goal controls] {actions:?}")
                        }
                        AgentEvent::ElicitationRequest {
                            message, fields, ..
                        } => eprintln!("[elicitation] {message} fields={}", fields.len()),
                        AgentEvent::TurnStarted => eprintln!("[turn started]"),
                        AgentEvent::TurnEnded { reason } => {
                            eprintln!("\n[turn ended: {reason:?}]")
                        }
                        AgentEvent::Failed(error) => eprintln!("[failed] {error}"),
                        AgentEvent::Notice(message) => eprintln!("[notice] {message}"),
                        AgentEvent::SessionStarted {
                            session_id,
                            resumed,
                            ..
                        } => eprintln!("[session started] {session_id} resumed={resumed}"),
                        AgentEvent::SessionLost => eprintln!("[session lost]"),
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

    // ── O2: slash コマンド一覧・会話名・目標（待機中の更新） ──

    use serde_json::json;

    fn mapped(update: v1::SessionUpdate) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        session_update_events(update, |event| events.push(event));
        events
    }

    /// 目標への操作の広告（Codex の initialize `_meta.goal`・O17）: 入口は拡張メソッドだけ受け、
    /// 知らない操作（set など）は捨てる。操作が 1 つも無ければ出さない。
    #[test]
    fn goal_controls_come_from_the_initialize_meta() {
        let meta = |value: serde_json::Value| -> v1::Meta {
            serde_json::from_value(value).expect("オブジェクト")
        };
        let codex = meta(json!({"goal": {
            "version": 1,
            "controlMethod": "_session/goal",
            "actions": ["set", "pause", "resume", "clear"]
        }}));
        assert_eq!(
            goal_controls(Some(&codex)),
            Some((
                "_session/goal".to_string(),
                vec![GoalAction::Pause, GoalAction::Resume, GoalAction::Clear]
            ))
        );
        let standard =
            meta(json!({"goal": {"controlMethod": "session/prompt", "actions": ["pause"]}}));
        assert_eq!(
            goal_controls(Some(&standard)),
            None,
            "標準のメソッドは呼ばない"
        );
        let only_set =
            meta(json!({"goal": {"controlMethod": "_session/goal", "actions": ["set"]}}));
        assert_eq!(goal_controls(Some(&only_set)), None);
        assert_eq!(goal_controls(None), None);
        assert_eq!(GoalAction::Pause.as_str(), "pause");
    }

    /// サブエージェントの手順（O17）: Claude が `_meta.claudeCode.parentToolUseId` に載せる親の id を
    /// 開始にも更新にも写す。印の無い手順・空の id は親なし。
    #[test]
    fn subagent_steps_carry_their_parent_tool_call() {
        let child: v1::ToolCall = serde_json::from_value(json!({
            "toolCallId": "toolu_child",
            "title": "Read src/lib.rs",
            "kind": "read",
            "_meta": {"claudeCode": {"parentToolUseId": "toolu_task", "toolName": "Read"}}
        }))
        .expect("ToolCall をパースできる");
        let events = mapped(v1::SessionUpdate::ToolCall(child));
        let [AgentEvent::ToolStarted(started)] = events.as_slice() else {
            panic!("ToolStarted 1 つ: {events:?}");
        };
        assert_eq!(started.parent.as_deref(), Some("toolu_task"));

        let update: v1::ToolCallUpdate = serde_json::from_value(json!({
            "toolCallId": "toolu_child",
            "status": "completed",
            "_meta": {"claudeCode": {"parentToolUseId": "toolu_task"}}
        }))
        .expect("ToolCallUpdate をパースできる");
        let events = mapped(v1::SessionUpdate::ToolCallUpdate(update));
        let [AgentEvent::ToolUpdated(updated)] = events.as_slice() else {
            panic!("ToolUpdated 1 つ: {events:?}");
        };
        assert_eq!(updated.parent.as_deref(), Some("toolu_task"));

        for meta in [json!({}), json!({"claudeCode": {"parentToolUseId": ""}})] {
            let top: v1::ToolCall = serde_json::from_value(json!({
                "toolCallId": "toolu_top",
                "title": "Task",
                "_meta": meta
            }))
            .expect("ToolCall をパースできる");
            let events = mapped(v1::SessionUpdate::ToolCall(top));
            let [AgentEvent::ToolStarted(started)] = events.as_slice() else {
                panic!("ToolStarted 1 つ: {events:?}");
            };
            assert!(started.parent.is_none(), "親の印が無ければ普通の手順");
        }
    }

    /// `AvailableCommandsUpdate` は一覧まるごと（ヒント付き・ヒント無し）で `Commands` になる。
    /// `SessionInfoUpdate` は title の「値 / null / 無し」を区別し、`_meta.goal` は目標になる。
    #[test]
    fn session_updates_map_commands_titles_and_goals() {
        let commands = mapped(v1::SessionUpdate::AvailableCommandsUpdate(
            v1::AvailableCommandsUpdate::new(vec![
                v1::AvailableCommand::new("compact", "会話を要約して文脈を空ける"),
                v1::AvailableCommand::new("review", "変更をレビューする").input(
                    v1::AvailableCommandInput::Unstructured(v1::UnstructuredCommandInput::new(
                        "optional review instructions",
                    )),
                ),
                v1::AvailableCommand::new("mcp:github:pr", "PR を作る").input(
                    v1::AvailableCommandInput::Unstructured(v1::UnstructuredCommandInput::new(
                        "  ",
                    )),
                ),
            ]),
        ));
        let [AgentEvent::Commands(commands)] = commands.as_slice() else {
            panic!("Commands 1 つ: {commands:?}");
        };
        assert_eq!(
            commands,
            &vec![
                SlashCommand {
                    name: "compact".into(),
                    description: "会話を要約して文脈を空ける".into(),
                    hint: None,
                },
                SlashCommand {
                    name: "review".into(),
                    description: "変更をレビューする".into(),
                    hint: Some("optional review instructions".into()),
                },
                SlashCommand {
                    name: "mcp:github:pr".into(),
                    description: "PR を作る".into(),
                    hint: None,
                },
            ],
            "空白だけのヒントはヒント無し扱い"
        );

        let titled = mapped(v1::SessionUpdate::SessionInfoUpdate(
            v1::SessionInfoUpdate::new().title("ログイン修正"),
        ));
        assert!(
            matches!(titled.as_slice(), [AgentEvent::TitleChanged(Some(title))] if title == "ログイン修正"),
            "{titled:?}"
        );
        let cleared = mapped(v1::SessionUpdate::SessionInfoUpdate(
            v1::SessionInfoUpdate::new().title(None),
        ));
        assert!(
            matches!(cleared.as_slice(), [AgentEvent::TitleChanged(None)]),
            "null は名前を消す: {cleared:?}"
        );

        let mut goal_meta = serde_json::Map::new();
        goal_meta.insert(
            "goal".into(),
            json!({"objective": " テストを緑にする ", "status": "paused", "tokensUsed": 12}),
        );
        let goal = mapped(v1::SessionUpdate::SessionInfoUpdate(
            v1::SessionInfoUpdate::new().meta(goal_meta),
        ));
        assert_eq!(
            goal.len(),
            1,
            "title の無い通知で会話名を消さない（目標だけ）: {goal:?}"
        );
        assert!(
            matches!(
                &goal[0],
                AgentEvent::GoalChanged(Some(AgentGoal { objective, status: GoalStatus::Paused }))
                    if objective == "テストを緑にする"
            ),
            "{goal:?}"
        );
        let mut goal_cleared = serde_json::Map::new();
        goal_cleared.insert("goal".into(), serde_json::Value::Null);
        let cleared_goal = mapped(v1::SessionUpdate::SessionInfoUpdate(
            v1::SessionInfoUpdate::new().meta(goal_cleared),
        ));
        assert!(
            matches!(cleared_goal.as_slice(), [AgentEvent::GoalChanged(None)]),
            "{cleared_goal:?}"
        );
        // Claude のファイル変更報告のような `_meta` だけの通知は何も出さない。
        let mut other_meta = serde_json::Map::new();
        other_meta.insert("jetbrains".into(), json!({"air": {}}));
        assert!(mapped(v1::SessionUpdate::SessionInfoUpdate(
            v1::SessionInfoUpdate::new().meta(other_meta)
        ))
        .is_empty());
    }

    /// `usage_update`（claude-agent-acp が送る実際の形）: 文脈の使用量に加えて、ターンの結果の
    /// `cost`（会話の累計）と、`rate_limit_event` の中継 `_meta["_claude/rateLimit"]` を捨てずに流す。
    #[test]
    fn usage_updates_carry_cost_and_rate_limits() {
        let result: v1::SessionUpdate = serde_json::from_value(json!({
            "sessionUpdate": "usage_update",
            "used": 52_000,
            "size": 200_000,
            "cost": {"amount": 1.25, "currency": "USD"},
            "_meta": {"_claude/model": "claude-opus-5[1m]"}
        }))
        .expect("usage_update を読める");
        let events = mapped(result);
        assert!(
            matches!(
                events.as_slice(),
                [
                    AgentEvent::Usage { used: 52_000, size: 200_000 },
                    AgentEvent::SessionCost { amount, currency },
                ] if (*amount - 1.25).abs() < 1e-9 && currency == "USD"
            ),
            "{events:?}"
        );

        let rate_limit: v1::SessionUpdate = serde_json::from_value(json!({
            "sessionUpdate": "usage_update",
            "used": 52_000,
            "size": 200_000,
            "_meta": {
                "_claude/rateLimit": {
                    "status": "allowed_warning",
                    "resetsAt": 1_790_000_000,
                    "rateLimitType": "five_hour",
                    "utilization": 0.85,
                    "unifiedWindows": {
                        "five_hour": {"utilization": 0.85, "resetsAt": 1_790_000_000},
                        "seven_day": {"utilization": 0.2, "resetsAt": 1_790_400_000}
                    }
                },
                "_claude/model": "claude-opus-5[1m]"
            }
        }))
        .expect("usage_update を読める");
        let events = mapped(rate_limit);
        let [AgentEvent::Usage { .. }, AgentEvent::RateLimits(limits)] = events.as_slice() else {
            panic!("Usage と RateLimits: {events:?}");
        };
        assert_eq!(limits.status, Some(usage::LimitStatus::Warning));
        assert_eq!(limits.windows.len(), 2);

        // 壊れた cost（負）は流さない。rate limit の無い `_meta` も何も足さない。
        let odd: v1::SessionUpdate = serde_json::from_value(json!({
            "sessionUpdate": "usage_update",
            "used": 1,
            "size": 2,
            "cost": {"amount": -3.0, "currency": "USD"},
            "_meta": {"_claude/origin": {"kind": "task-notification"}}
        }))
        .expect("usage_update を読める");
        assert!(
            matches!(
                mapped(odd).as_slice(),
                [AgentEvent::Usage { used: 1, size: 2 }]
            ),
            "負のコストは捨てる"
        );
    }

    /// 偽エージェント: prompt を受けると、ターン中に rate limit とコストを送り、`_meta.quota` 付きで
    /// 応答する（claude-agent-acp の `turnOutcome` と同じ形）。
    const AGENT_THAT_REPORTS_USAGE: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

def update(sid, body):
    send({"jsonrpc": "2.0", "method": "session/update",
          "params": {"sessionId": sid, "update": body}})

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1)}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "sess-1"}})
    elif method == "session/prompt":
        sid = params["sessionId"]
        update(sid, {"sessionUpdate": "usage_update", "used": 900, "size": 200000,
                     "_meta": {"_claude/rateLimit": {"status": "allowed", "rateLimitType": "five_hour",
                                                     "resetsAt": 1790000000, "utilization": 0.42}}})
        update(sid, {"sessionUpdate": "agent_message_chunk",
                     "content": {"type": "text", "text": "done"}})
        update(sid, {"sessionUpdate": "usage_update", "used": 1200, "size": 200000,
                     "cost": {"amount": 0.37, "currency": "USD"}})
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "stopReason": "end_turn",
            "usage": {"inputTokens": 10, "outputTokens": 20, "cachedReadTokens": 30,
                      "cachedWriteTokens": 40, "totalTokens": 100},
            "_meta": {"quota": {
                "token_count": {"totalTokens": 100, "inputTokens": 10, "cachedInputTokens": 30,
                                "cachedWriteTokens": 40, "outputTokens": 20,
                                "reasoningOutputTokens": 0},
                "model_usage": [{"model": "claude-opus-5[1m]", "token_count": {
                    "totalTokens": 100, "inputTokens": 10, "cachedInputTokens": 30,
                    "cachedWriteTokens": 40, "outputTokens": 20, "reasoningOutputTokens": 0}}]}}}})
        sys.exit(0)
"#;

    /// ターンの終わりの `_meta.quota` は `TurnUsage` になり、`TurnEnded` より先に届く。ターン中の
    /// コストと rate limit も流れる。
    #[test]
    fn turn_usage_arrives_before_turn_end() {
        let Some((outcome, events)) = run_fake_agent_until_session_ends(
            AGENT_THAT_REPORTS_USAGE,
            SessionPreferences::default(),
            Some("やって"),
        ) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        outcome.expect("セッションは正常終了する");
        let usage_at = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::TurnUsage(usage::TurnTokens {
                        input: 10,
                        output: 20,
                        cached_read: 30,
                        cached_write: 40,
                        total: 100,
                    })
                )
            })
            .unwrap_or_else(|| panic!("TurnUsage が届く: {events:?}"));
        let ended_at = events
            .iter()
            .position(|event| matches!(event, AgentEvent::TurnEnded { .. }))
            .unwrap_or_else(|| panic!("TurnEnded が届く: {events:?}"));
        assert!(
            usage_at < ended_at,
            "TurnUsage は TurnEnded の前: {events:?}"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::SessionCost { amount, .. } if (*amount - 0.37).abs() < 1e-9
            )),
            "{events:?}"
        );
        assert!(
            events.iter().any(
                |event| matches!(event, AgentEvent::RateLimits(limits) if limits.windows.len() == 1)
            ),
            "{events:?}"
        );
    }

    /// slash コマンドは本文 1 ブロックだけで送る（先頭 = 末尾 = `/…`）。Claude は末尾、Codex は
    /// 先頭のブロックをコマンドとして読むため、1 ブロックでないとどちらかで効かない。
    #[test]
    fn a_slash_command_travels_as_a_single_text_block() {
        let (event_tx, _event_rx) = mpsc::unbounded();
        let blocks = prompt_blocks("/review 認証まわり".into(), Vec::new(), true, &event_tx);
        let [v1::ContentBlock::Text(text)] = blocks.as_slice() else {
            panic!("本文 1 ブロックだけ: {blocks:?}");
        };
        assert_eq!(text.text, "/review 認証まわり");
    }

    /// 偽エージェントを立て、prompt を送らずに `until` が満たされるまで（最長 10 秒）イベントを
    /// 集め、送信路を閉じてセッションを終わらせる（待機中に届く更新の検証用）。
    fn collect_idle_events(
        script: &str,
        until: impl Fn(&[AgentEvent]) -> bool,
    ) -> Option<Vec<AgentEvent>> {
        use futures::future::FutureExt as _;
        let command = fake_agent_command(script)?;
        let (command_tx, command_rx) = mpsc::unbounded::<SessionCommand>();
        let (event_tx, mut event_rx) = mpsc::unbounded();
        Some(futures::executor::block_on(async move {
            let session = run_session(command, SessionPreferences::default(), command_rx, event_tx);
            let collect = async move {
                let mut seen = Vec::new();
                // 上限は偽エージェント（python）の起動込み。テストが並んで重い Windows のランナーでは
                // 起動だけで 10 秒近くかかり、届く前に打ち切っていた（PR #30 の check-windows）。
                // 条件がそろえばすぐ抜けるので、長くしても通る時は遅くならない（固まった時の保険）。
                let deadline =
                    blocking::unblock(|| std::thread::sleep(Duration::from_secs(60))).fuse();
                futures::pin_mut!(deadline);
                loop {
                    let next = event_rx.next().fuse();
                    futures::pin_mut!(next);
                    futures::select! {
                        event = next => match event {
                            Some(event) => {
                                seen.push(event);
                                if until(&seen) {
                                    break;
                                }
                            }
                            None => break,
                        },
                        () = deadline => break,
                    }
                }
                drop(command_tx);
                while let Some(event) = event_rx.next().await {
                    seen.push(event);
                }
                seen
            };
            let (outcome, seen) = futures::join!(session, collect);
            outcome.expect("セッションは正常終了する");
            seen
        }))
    }

    /// 偽エージェント: `session/new` に答えた直後（prompt は来ない）にコマンド一覧を 2 回
    /// （2 回目は全量の差し替え）、`_meta` だけの `session_info_update`、会話名を送る。
    const AGENT_WITH_IDLE_UPDATES: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

def update(sid, body):
    send({"jsonrpc": "2.0", "method": "session/update",
          "params": {"sessionId": sid, "update": body}})

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1)}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "sess-1"}})
        update("sess-1", {"sessionUpdate": "available_commands_update",
                          "availableCommands": [
                              {"name": "compact", "description": "Compact", "input": None},
                              {"name": "review", "description": "Review",
                               "input": {"hint": "instructions"}}]})
        update("sess-1", {"sessionUpdate": "available_commands_update",
                          "availableCommands": [
                              {"name": "compact", "description": "Compact", "input": None},
                              {"name": "goal", "description": "Set a goal",
                               "input": {"hint": "[<objective>|clear]"}},
                              {"name": "mcp:github:pr", "description": "Open a PR"}]})
        update("sess-1", {"sessionUpdate": "session_info_update",
                          "_meta": {"jetbrains": {"air": {}}}})
        update("sess-1", {"sessionUpdate": "session_info_update",
                          "title": "ログイン修正", "updatedAt": "2026-09-26T00:00:00Z"})
"#;

    /// 回帰テスト（O2）: 待機中（ターンの外）に届く更新も読む。以前の待機ループは command と
    /// EOF しか待たず、`session/new` 直後のコマンド一覧も、ターン後に付く会話名も、次の prompt を
    /// 送るまで UI に届かなかった。prompt を 1 通も送らずに両方が届くことを確かめる。
    #[test]
    fn commands_and_title_reach_the_ui_while_idle() {
        let Some(events) = collect_idle_events(AGENT_WITH_IDLE_UPDATES, |events| {
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::TitleChanged(_)))
        }) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let lists: Vec<Vec<String>> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Commands(commands) => Some(
                    commands
                        .iter()
                        .map(|command| command.name.clone())
                        .collect(),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(
            lists,
            vec![
                vec!["compact".to_string(), "review".to_string()],
                vec![
                    "compact".to_string(),
                    "goal".to_string(),
                    "mcp:github:pr".to_string()
                ],
            ],
            "一覧は届いた順に、毎回全量で流れる: {events:?}"
        );
        let titles: Vec<Option<String>> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TitleChanged(title) => Some(title.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            titles,
            vec![Some("ログイン修正".to_string())],
            "`_meta` だけの通知は名前を消さない: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::TurnStarted)),
            "prompt は送っていない: {events:?}"
        );
    }

    /// 偽エージェント: `session/load` の最中に履歴の本文・コマンド一覧・会話名を流してから応答する。
    const AGENT_THAT_REPLAYS_STATE_ON_LOAD: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

def update(sid, body):
    send({"jsonrpc": "2.0", "method": "session/update",
          "params": {"sessionId": sid, "update": body}})

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"protocolVersion": params.get("protocolVersion", 1),
                         "agentCapabilities": {"loadSession": True}}})
    elif method == "session/load":
        sid = params["sessionId"]
        update(sid, {"sessionUpdate": "agent_message_chunk",
                     "content": {"type": "text", "text": "old history"}})
        update(sid, {"sessionUpdate": "available_commands_update",
                     "availableCommands": [{"name": "compact", "description": "Compact"}]})
        update(sid, {"sessionUpdate": "session_info_update", "title": "前回の会話"})
        send({"jsonrpc": "2.0", "id": rid, "result": {}})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
        sys.exit(0)
"#;

    /// `session/load` の再生は本文を捨てる（二重表示しない）が、状態（コマンド一覧・会話名）は
    /// 捨てずに UI へ流す。
    #[test]
    fn resume_keeps_replayed_state_but_drops_replayed_history() {
        let preferences = SessionPreferences {
            resume: Some("prev-1".into()),
            ..SessionPreferences::default()
        };
        let Some((outcome, events)) = run_fake_agent_until_session_ends(
            AGENT_THAT_REPLAYS_STATE_ON_LOAD,
            preferences,
            Some("続き"),
        ) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        outcome.expect("セッションは正常終了する");
        assert!(
            !events.iter().any(
                |event| matches!(event, AgentEvent::AgentChunk(text) if text == "old history")
            ),
            "再生された本文は流さない: {events:?}"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::Commands(commands) if commands.len() == 1 && commands[0].name == "compact"
            )),
            "再生中のコマンド一覧は流す: {events:?}"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::TitleChanged(Some(title)) if title == "前回の会話"
            )),
            "再生中の会話名も流す: {events:?}"
        );
    }
}

/// remote での解決は「見つからない」と「探せなかった」を混ぜない（2026-09-08 の実バグ:
/// SSH 再接続の失敗が「claude-agent-acp が見つかりません」になっていた）。
#[cfg(test)]
mod resolve_on_host_tests {
    use super::*;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    /// remote の `command -v` の返事を台本で決める偽 host。
    enum Probe {
        /// SSH が切れていて探しに行けない。
        Unreachable,
        /// 探せたが無い。
        Missing,
        /// 見つかった。
        Found,
    }

    struct FakeRemote {
        inner: Arc<dyn Host>,
        probe: Probe,
        /// (流した script, 再送可で来たか)
        calls: Mutex<Vec<(String, bool)>>,
    }

    impl FakeRemote {
        fn new(probe: Probe) -> Self {
            Self {
                inner: LocalHost::shared(),
                probe,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn answer(&self, spec: &CommandSpec, retry_safe: bool) -> Result<host::CommandOutput> {
            let script = spec.args.get(1).cloned().unwrap_or_default();
            self.calls
                .lock()
                .expect("calls lock")
                .push((script, retry_safe));
            match self.probe {
                Probe::Unreachable => anyhow::bail!(
                    "Remote SSH 再接続に失敗: OpenSSH ControlMaster の接続に失敗: exit status: 255"
                ),
                Probe::Missing => Ok(host::CommandOutput {
                    status_code: Some(1),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                }),
                Probe::Found => Ok(host::CommandOutput {
                    status_code: Some(0),
                    stdout: b"/remote/bin/claude-agent-acp\n".to_vec(),
                    stderr: Vec::new(),
                }),
            }
        }
    }

    impl Host for FakeRemote {
        fn id(&self) -> &str {
            "fake-remote"
        }
        fn display_name(&self) -> &str {
            "fake-remote"
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
        fn run_command(&self, spec: &CommandSpec) -> Result<host::CommandOutput> {
            self.answer(spec, false)
        }
        fn run_command_retry_safe(&self, spec: &CommandSpec) -> Result<host::CommandOutput> {
            self.answer(spec, true)
        }
        fn spawn_process(&self, spec: &CommandSpec) -> Result<host::HostProcess> {
            self.inner.spawn_process(spec)
        }
        fn terminal_launch(&self, cwd: &Path) -> Result<Option<host::TerminalLaunch>> {
            self.inner.terminal_launch(cwd)
        }
    }

    fn claude() -> &'static AgentKind {
        AgentKind::by_label("Claude Code").expect("Claude Code は組み込みカタログにある")
    }

    /// SSH が切れているときは `Err`（原因文つき）。`Ok(None)`＝「見つからない」にしない。
    #[test]
    fn unreachable_remote_is_an_error_not_missing() {
        let host = FakeRemote::new(Probe::Unreachable);
        let message = match claude().resolve_command_on(&host, "/work", None, None) {
            Err(error) => format!("{error:#}"),
            Ok(resolved) => panic!(
                "探しに行けないなら Err（見つかった扱い: {}）",
                resolved.is_some()
            ),
        };
        assert!(message.contains("探せない"), "文脈が付く: {message}");
        assert!(
            message.contains("exit status: 255"),
            "原因文が残る: {message}"
        );
    }

    /// 探せたが無いなら `Ok(None)`（bin も npx も無い）。
    #[test]
    fn missing_on_remote_is_none() {
        let host = FakeRemote::new(Probe::Missing);
        let resolved = claude()
            .resolve_command_on(&host, "/work", None, None)
            .expect("探せている");
        assert!(resolved.is_none());
        let calls = host.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 2, "bin → npx の順に 2 回探す: {calls:?}");
    }

    /// 見つかったら remote のパスで組む。探索は**再送可**で流す（切断直後の 1 発目で落とさない）。
    #[test]
    fn found_on_remote_uses_retry_safe_probe() {
        let host = FakeRemote::new(Probe::Found);
        let resolved = claude()
            .resolve_command_on(&host, "/work", None, None)
            .expect("探せている")
            .expect("見つかる");
        assert_eq!(resolved.path, PathBuf::from("/remote/bin/claude-agent-acp"));
        let calls = host.calls.lock().expect("calls lock");
        assert!(
            calls.iter().all(|(_, retry_safe)| *retry_safe),
            "command -v は再送可で送る: {calls:?}"
        );
    }
}
