//! settings_core — 設定の3層マージ（default → user → project）。GPUI 非依存・テスト可能。
//!
//! ARCHITECTURE §7: user = `~/Library/Application Support/necoder/settings.json`、
//! project = `.necoder/settings.json`。後ろのレイヤが前を**深く**上書きする（オブジェクトは再帰マージ、
//! スカラ・配列は置換）。マージ後の JSON を [`Settings`] にデシリアライズする（欠けたキーは型の既定）。

use anyhow::{Context as _, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 密度（UI-SPEC §1.4）。行高・パディングの基準。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Compact,
    Cozy,
}

impl Density {
    /// 行高（UI-SPEC §1.4: compact 23 / cozy 27）。
    pub fn line_height(self) -> f32 {
        match self {
            Density::Compact => 23.0,
            Density::Cozy => 27.0,
        }
    }
}

/// レール（最左アクティビティバー）の各アイコンの表示。全て既定 true・settings で個別に消せる。
/// 例: `.necoder/settings.json` に `{ "rail": { "terminal": false } }` でターミナルアイコンを隠す。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct RailSettings {
    pub explorer: bool,
    pub search: bool,
    pub git: bool,
    pub agent: bool,
    pub terminal: bool,
    /// Todo ボード（.necoder/todos.md・M12-10）。
    pub todos: bool,
    /// リモート SSH（~/.ssh/config のホストへ接続・#2）。
    pub remote: bool,
    /// Fleet（多エージェントの面・FLEET-V2）。レールから Editor ⇄ Fleet を切り替える。
    pub fleet: bool,
    /// Chat（プロジェクトに紐づかない会話の面・`docs/CHAT.md`）。
    pub chat: bool,
}

/// Chat モードの設定（`docs/CHAT.md` §2.2 / §4.1）。
/// 例: `{ "chat": { "directory": "~/Chats", "instructions": "敬語は使わない" } }`
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ChatSettings {
    /// チャットのフォルダを並べる場所。空なら書類フォルダの `necoder/`（`~/` は展開する）。
    /// 同期フォルダを避けたい人・別ドライブに置きたい人のための逃げ道。
    pub directory: String,
    /// どのチャットにも効かせる指示。Chat 用のシステムプロンプトの末尾に足す。
    pub instructions: String,
    /// 使っていないチャットのエージェントを止めるまでの分数（`0` = 止めない）。
    /// チャットは何本も開いたままになりがちで、1 本ごとにエージェントのプロセスが常駐する。
    /// 止めても会話は失われない（次の送信で `session/load` が同じ会話を引き継ぐ）。
    pub idle_stop_minutes: u64,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            directory: String::new(),
            instructions: String::new(),
            idle_stop_minutes: 10,
        }
    }
}

impl Default for RailSettings {
    fn default() -> Self {
        Self {
            explorer: true,
            search: true,
            git: true,
            agent: true,
            terminal: true,
            todos: true,
            remote: true,
            fleet: true,
            chat: true,
        }
    }
}

/// エージェントごとの sticky 既定（モデル / 思考量 / 権限モード）。
/// **どのエージェントを使うか**（`default_agent`・§8）とは別レイヤ — こちらは「作業のたびに選び直したくない」
/// エージェントの**起動方法の上書き**（`agent_servers.<id>`）。
///
/// 版はレジストリ（`acp_client::registry`）と組み込みカタログが決めるが、**ユーザーが
/// necoder のリリースを待たずに先へ進める逃げ道**をここに置く。レジストリが落ちていても、
/// 新しい版を先に試したくても、これがあれば自力で回避できる。
///
/// **`custom` と `registry` を分ける理由**: 起動コマンドを持つのは `custom` だけにして、
/// 「レジストリ管理のエージェントのコマンドだけを半端に差し替える」形を作らせない。
/// 半端な上書きは、版とコマンドが食い違ったまま動く状態を生む。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentServerSetting {
    /// 起動を丸ごと自前で決める。necoder はこのコマンドをそのまま起動する（版の解決もしない）。
    Custom {
        /// 実行するコマンド（絶対パス、または PATH 上の名前）。
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// レジストリ管理のまま、環境変数だけ足す。**コマンドと版はレジストリが持つ**。
    Registry {
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
}

impl AgentServerSetting {
    /// この設定が足す環境変数（どちらの形でも持つ）。
    pub fn env(&self) -> &BTreeMap<String, String> {
        match self {
            Self::Custom { env, .. } | Self::Registry { env } => env,
        }
    }

    /// settings.json に書く形（`type` つき・空の `args` / `env` も書く）。
    pub fn to_json(&self) -> Value {
        match self {
            Self::Custom { command, args, env } => serde_json::json!({
                "type": "custom",
                "command": command,
                "args": args,
                "env": env,
            }),
            Self::Registry { env } => serde_json::json!({ "type": "registry", "env": env }),
        }
    }
}

/// 環境変数の欄（1 行に 1 つ `KEY=VALUE`・空行と `#` で始まる行は読まない・O16）を読む。名前は
/// 英字か `_` で始まり英数字と `_` だけ。値はそのまま（引用符も外さない）。誤りは行番号つきで返す。
pub fn parse_env_lines(text: &str) -> std::result::Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!(
                "{} 行目: KEY=VALUE の形ではありません: {line}",
                number + 1
            ));
        };
        let name = name.trim();
        let valid = name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
        if !valid {
            return Err(format!(
                "{} 行目: 変数の名前に使えない文字があります: {name}",
                number + 1
            ));
        }
        env.insert(name.to_string(), value.trim().to_string());
    }
    Ok(env)
}

/// [`parse_env_lines`] の逆（欄に出す形・名前の順）。
pub fn env_lines(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 引数の欄（1 行に 1 つ・前後の空白は落とす・空行は読まない）を読む。引用符は付けずに書く
/// （行がそのまま 1 つの引数）。
pub fn parse_arg_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// よく使うコマンド 1 つ（`quick_commands` の要素・O25）。ターミナルのドックの ▶ に並び、押すと
/// 新しい端末で走らせる。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QuickCommandSetting {
    /// ボタンに出す名前（例 `"開発サーバ"`）。
    pub name: String,
    /// 走らせるコマンド（例 `"npm run dev"`）。シェルに打つのと同じ。
    pub command: String,
}

/// MCP サーバ 1 件の設定（`mcp_servers.<name>`）。
///
/// ACP は「どの MCP サーバへ繋ぐか」を**クライアント（necoder）が決める**プロトコルで、
/// エージェント側の設定ファイル（`~/.codex/config.toml` 等）はセッションに現れない。
/// necoder が渡した分だけがエージェントから見える（`acp_client::mcp`）。
///
/// 書き方は 2 通り:
/// - **自前定義**（`command` か `url` を書く）— necoder がこの内容でエージェントへ渡す。既定 on。
/// - **有効/無効だけ**（`enabled` のみ）— 他ツール（Codex CLI / Claude Code / Cursor）の設定から
///   発見したサーバの on/off を決める行。発見しただけのサーバは**既定 off**（他人の設定を根拠に
///   子プロセスを起こしたり課金されるリモートサーバへ繋いだりしない）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct McpServerSetting {
    /// 明示的な有効/無効。`None` = 出所ごとの既定（自前定義は on・発見は off）。
    pub enabled: Option<bool>,
    /// 伝送方式（`stdio` / `http` / `sse`）。省略時は `url` があれば http、無ければ stdio。
    /// 他ツールの設定ファイルに合わせて `type` でも書ける。
    #[serde(alias = "type")]
    pub transport: Option<String>,
    /// stdio: 起動するコマンド（絶対パス、または PATH 上の名前）。
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// http / sse: 接続先 URL。
    pub url: Option<String>,
    /// http / sse: 付けるヘッダ。値の `${VAR}` は起動時に環境変数へ展開する
    /// （未設定なら**そのサーバを渡さない**＝嘘のトークンで繋ぎに行かない）。
    pub headers: BTreeMap<String, String>,
}

/// 解決済み設定（全レイヤをマージ後に得る）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// テーマ名（theme_core の組み込み名 or ユーザーテーマ）。
    pub theme: String,
    pub density: Density,
    pub font_size: f32,
    pub tab_size: usize,
    /// soft wrap（折り返し表示）。⌥Z で一時トグルもできる。
    pub soft_wrap: bool,
    /// 保存時に LSP フォーマットをかける（対応言語のみ・M11）。
    pub format_on_save: bool,
    /// 自動保存（`"off"` = ⌘S だけ・既定 / `"focus_change"` = 他へ移った時 / `"after_delay"` = 手を
    /// 止めて 1 秒たった時と他へ移った時・O26）。自動保存ではフォーマットしない（打っている途中の行を
    /// 動かさない）。解釈は [`Settings::auto_save_mode`]（知らない値は保存しない側に倒す）。
    pub auto_save: String,
    /// プレビュータブ（既定 on・O26）。エクスプローラで 1 回クリックしたファイルは、次に 1 回
    /// クリックしたファイルで置き換わるタブ（名前が斜体）で開く。編集・ダブルクリック・ピン留めで
    /// 普通のタブになる。off = 常に普通のタブ（以前の挙動）。
    pub preview_tabs: bool,
    /// UI ロケール。`None` = OS 追従。
    pub locale: Option<String>,
    /// エージェント composer で **Enter を送信に使うか**。
    /// `false`（既定）= Enter は改行・⌘Enter で送信（日本語 IME の変換確定 Enter で誤送信しない安全側）。
    /// `true` = Enter で送信・Shift+Enter で改行（チャット風。IME 変換中は送信しない）。
    pub submit_on_enter: bool,
    /// エージェントの新規スレッドに、最初のやり取りから AI が自動でタイトルを付けるか（#6・既定 on）。
    /// 既定名（"スレッドN"）のまま・手動改名していないスレッドだけが対象。無効なら既定名のまま。
    pub agent_auto_name: bool,
    /// エージェントのセッションを**送信を待たずに**先に張るか（既定 on）。
    /// ACP はセッションが開くまでモデル/モード一覧を広告しないので、off にすると composer 下の
    /// ピルは最初の送信まで空のまま（Zed は常に先張りする側）。off の利点は idle メモリ:
    /// 見ているタブごとにエージェントのプロセスが 1 本立たなくなる。
    pub agent_prewarm: bool,
    /// ターン完了の通知音。同梱の猫の声（[`SOUND_VOICES`]・既定 `"nyaan"`）/ `"system"`（OS の音）/
    /// `"off"` / 任意のファイルパス（`~/` 可）。裏の窓で走らせた作業の完了に気づくための音
    /// （`docs/BACKGROUND.md` の原点痛点）。**見ている画面では鳴らさない**。
    pub sound_done: String,
    /// 入力待ち（承認・質問で止まった）の通知音。値の取り方は [`Settings::sound_done`] と同じ。
    /// 完了とは違う音（別の声でもいい）を当てて、耳だけで「終わった」と「呼ばれている」を区別する。
    pub sound_waiting: String,
    /// 通知音の大きさ（0〜100 %・既定 100 = 音源そのまま・O13）。0 は鳴らさない（声は選んだまま）。
    pub sound_volume: u64,
    /// OS のデスクトップ通知（O12・既定 on）。ターンの完了 / 失敗・承認待ち・質問待ちを、
    /// **その窓を見ていない時だけ**通知センターへ出す（見ている時は右下のトーストで足りる）。
    /// ミュートしたスレッドは出さない。押すとそのスレッドへ飛ぶ。
    pub system_notifications: bool,
    /// 装飾的な動きを静止するアクセシビリティ設定。GPUI の `reduce_motion` へ接続し、
    /// スピナー・fade・マスコットなどの継続アニメーションを静止画として描く。
    pub reduce_motion: bool,
    /// Tier 2 遷移スナップショット（✳ 1 行要約・FLEET-CONTROL-PLAN P4・既定 on）。
    /// Done/Failed 遷移時に既定 Agent の oneshot CLI で 1 行生成する。オフでも Tier 1（決定論）は出続ける。
    pub tier2_summaries: bool,
    /// Captain に任命するエージェント表示名（FLEET-V2 §5.7・None = 未任命）。
    /// 任命は settings.json の明示編集（既定ドリフト禁止の原則・DECISIONS §8）。
    /// 任命すると Blocked(15s)/Done/Failed 遷移で IntegrationSpace の Captain スレッドが 1 ターン起きる。
    pub captain_agent: Option<String>,
    /// スレッドタブの見せ方（"bar" 横タブ / "list" 縦リスト）。Agent パネルのスイッチャがここへ保存し、
    /// 次の起動でも保つ。設定画面のトグル化は後続（真実はこの値・画面はこれを操作するだけ）。
    pub agent_tabs_view: String,
    /// 作業ペイン / ファイルタブの既定位置。各 Fleet ペインは個別に上書きできる。
    pub work_tabs_position: String,
    /// 新規スレッドの既定 AI エージェント（表示名。`acp_client::AGENT_LABELS` のいずれか）。
    /// **変更は Settings 画面（★ 既定にする）でのみ** — composer のピルはこのグローバル既定を書き換えない
    /// （哲学「自分で決めた既定はドリフトしない」・DECISIONS §8）。
    pub default_agent: String,
    /// エージェントごとに覚えた選択（`agent_id` → `config_id` → **value_id**）。
    /// 例: `{"claude": {"model": "opus[1m]", "effort": "xhigh", "mode": "bypassPermissions"}}`。
    ///
    /// **保存するのは ACP が広告する value_id だけ**（表示名も necoder 独自の綴りも入れない・2026-09-09）。
    /// 表示名は接続後の広告から引く。ここに表示名を混ぜると「保存した綴り」と「広告の綴り」が
    /// 食い違い、毎回エージェント既定へ落ちる（`docs/DECISIONS.md` の該当項）。
    ///
    /// キーはラベルでなく `AgentKind::id`。`config_id` は ACP のもの（Claude Code なら
    /// `model` / `effort` / `mode` / `fast`）で、necoder が知らない項目も素通しで持てる。
    /// `default_agent` は §8 のまま Settings 画面だけが変える — 「どの agent か」と「その agent の設定」を分離する。
    pub agent_config_defaults: BTreeMap<String, BTreeMap<String, String>>,
    /// エージェントの起動方法の上書き（necoder の `AgentKind::id` がキー。例 `"codex"`）。
    /// 空＝レジストリと組み込みカタログに従う（通常はこれ）。詳細は [`AgentServerSetting`]。
    pub agent_servers: BTreeMap<String, AgentServerSetting>,
    /// 新しいスレッドの権限モードの既定（O16）。`"default"`（既定）= エージェントの既定のモード（毎回
    /// 聞く側）/ `"bypass"` = 聞かずに進める（Yolo）: エージェントが広告するモードのうち「聞かない」もの
    /// （Claude Code の `bypassPermissions` など）で始める。持たないエージェントは既定のまま。
    /// ピルで選んだモード（`agent_config_defaults.<id>.mode`）があればそちらが勝つ。設定画面で選ぶと
    /// その記憶を全部消す（全エージェントを一括で揃える）。Chat のスレッドには効かない。
    pub agent_permission_default: String,
    /// 使わないエージェント（`AgentKind::id`・例 `["grok", "kimi"]`・O16）。選択肢（composer の
    /// エージェント・＋ Task の並べて比べる・パレットのエージェント別の新規スレッド）から外し、
    /// ログインの確かめ（CLI の status・ACP の試しのセッション）でも起こさない。設定 › エージェントの
    /// スイッチで出し入れする。既定のエージェントと Captain は外せない（画面が止める）。
    pub disabled_agents: Vec<String>,
    /// スレッドのセッションでエージェントへ渡す MCP サーバ（サーバ名がキー）。詳細は [`McpServerSetting`]。
    /// 空＝他ツールから発見した分だけが一覧に並び、どれも渡さない（有効化は明示だけ）。
    pub mcp_servers: BTreeMap<String, McpServerSetting>,
    /// worktree 削除の前に確認ダイアログを出すか（既定 on・2026-07-27）。
    /// **off にしても「失うものがある」ときは必ず確認する** — 未コミットの変更は git にも残らないので、
    /// 「二度と聞くな」の対象は *取り返しがつく* 削除に限る（DECISIONS の該当項）。
    pub confirm_worktree_delete: bool,
    /// ⌘Q・最後の窓を閉じる時の確認（`"running"` = 動いているものがある時だけ聞く・既定 /
    /// `"never"` = 聞かない）。エージェントも端末もアプリ本体の子なので、終了すると一緒に止まる。
    /// 解釈は [`Settings::quit_confirmation`]（知らない値は既定の側に倒す）。
    pub confirm_quit: String,
    /// エージェントが作業している間、コンピュータを寝かせないか（`"working"` = 作業中の間だけ・既定 /
    /// `"off"` = 止めない・O13）。止めるのは**放っておいた時のスリープ（idle sleep）だけ**で、画面は
    /// 普通に消え、自分で選んだスリープとノートの蓋を閉じた時のスリープは止めない。
    /// 解釈は [`Settings::keep_awake_mode`]（知らない値は止める側に倒す）。
    pub keep_awake: String,
    /// UI の書体（空 = 同梱の IBM Plex Sans JP・O27）。入っていない書体は OS の代わりの書体で描く。
    pub ui_font_family: String,
    /// コードの書体（空 = 同梱の Guguru Sans Code・O27）。エディタ・差分・パス、ターミナルの既定。
    /// 等幅の書体を書く。
    pub code_font_family: String,
    /// ターミナルの文字の大きさ（pt・既定 12.5 = エディタより少し小さい・O25）。範囲の外は 8〜32 に丸める。
    /// 変えると開いている端末もその場で描き直す（行と列は測り直してシェルへ伝わる）。
    pub terminal_font_size: f32,
    /// ターミナルのフォント（空 = コードの書体 `code_font_family`・O25）。等幅のフォントの名前を書く。
    pub terminal_font_family: String,
    /// ターミナルで遡れる行数（既定 10,000・上限 100,000・O25）。減らすと古い行から捨てる。
    pub terminal_scrollback: u64,
    /// ターミナルのカーソルの形（`"block"` 既定 / `"bar"` / `"underline"`・O25）。vim やシェルの設定が
    /// 形を指定すればそちらが勝つ（ここはその既定）。知らない値は `"block"`。
    pub terminal_cursor: String,
    /// ターミナルの配色ファイル（空 = 既定・アプリのテーマに合わせる・O25）。Ghostty のテーマ・
    /// Windows Terminal の scheme（JSON）・iTerm2 の `.itermcolors` から ANSI 16 色と文字 / 背景の色を読む。
    /// `~/` はホーム。設定を保存し直すと読み直す。
    pub terminal_color_scheme: String,
    /// statusbar で出さない項目（O27・右クリックで出し入れ）。`color` / `branch` / `diagnostics` /
    /// `terminal` / `activity` / `usage` / `cursor` / `encoding` / `language`。知らせ（SSH の接続・
    /// 承認待ち・クラッシュ・更新）は消せない。
    pub statusbar_hidden: Vec<String>,
    /// 手元のターミナルで開くシェル（空 = OS の既定・mac / Linux は `$SHELL`・Windows は pwsh → powershell・O25）。
    /// 名前（PATH から探す）か絶対パス。新しく開く端末から効く。SSH 先の端末は接続先のシェルのまま。
    pub terminal_shell: String,
    /// 上のシェルに渡す引数（例 `["-l"]`）。シェルが空なら使わない。
    pub terminal_shell_args: Vec<String>,
    /// よく使うコマンド（O25・`[{ "name": "開発サーバ", "command": "npm run dev" }]`）。ターミナルの
    /// ドックの ▶ に並ぶ。リポジトリの `.necoder/settings.json` にも書ける（書けば上書き）。
    pub quick_commands: Vec<QuickCommandSetting>,
    /// 旧 Fleet の互換設定。TaskSpace-first 以降は既定操作が常に `+ Task` なので挙動には使わない。
    /// 既存 settings.json を壊さず読めるよう schema field だけ保持する。
    pub fleet_agent_worktree: bool,
    /// HTML プレビュー（OS 標準 WebView）を非表示のまま放置したとき、自動破棄するまでの分数（既定 15・
    /// `0` = 自動破棄しない）。WebView は生きている間 数十〜数百 MB を別プロセスで握るため、
    /// idle メモリ予算を守る回収弁。破棄後の再表示は遅延再生成（初回表示と同じ経路）なので、
    /// ローカル HTML では失うものは実質スクロール位置だけ。
    pub html_preview_evict_minutes: u64,
    /// 使っていないスレッドのエージェントを止めるまでの分数（既定 15・`0` = 止めない）。
    /// エージェントは 1 本で 350〜700 MB を握り（実測 2026-09-19: npm のラッパー + アダプタの node +
    /// claude 本体）、スレッドを閉じるまで生き続けるので、開きっぱなしのスレッドが 20 本あれば 7 GB を超える。
    /// 止めても会話は失われない — 次の送信で `session/load` が同じ会話を引き継ぐ。
    /// **会話を引き継げるエージェント（`loadSession` を広告するもの）だけ**が対象。
    pub agent_idle_stop_minutes: u64,
    /// claude.ai アカウントに繋いだコネクタ（Figma・Google Calendar 等）を Claude のスレッドへ
    /// 読み込むか（既定 true = Claude Code の既定どおり）。読み込むとセッションの開始のたびに
    /// リモートの MCP サーバへ繋ぎに行く（実測: 初回応答 4.2 秒 → 切ると 1.8 秒・文脈 +6.7k トークン）。
    /// **Chat モードはこの設定に関わらず常に読み込まない**（necoder の MCP 設定で選んだ物だけを渡す原則）。
    pub claude_ai_connectors: bool,
    /// CLI（`necoder terminal send`）から端末へ文字を送ってよいか（既定 false）。
    /// 送ると、その端末のシェルやエージェントがそのまま実行する＝エージェントやスクリプトに
    /// 人の代わりにキーを打たせることになるので、人が設定画面で明示的に許可した時だけ効かせる。
    /// 一覧・読み取り・待機は許可なしで使える（何も起こさない）。
    pub allow_terminal_send: bool,
    /// レールのアイコン表示（アクティビティバー）。
    pub rail: RailSettings,
    /// Chat モード（`docs/CHAT.md`）。
    pub chat: ChatSettings,
    /// Fleet の初回導線（2 本目の Task を切った時の 1 回だけのトースト・FLEET-V2 §3.0）を出したか。
    /// 出したら `true` を書き込み、以後は**何も案内しない**（案内は 1 回・DECISIONS の静かさの原則）。
    pub fleet_hint_seen: bool,
    /// 初回オンボーディングを済ませたか（`false`＝初回で設定ホームが自動オープン・M12）。
    /// 「これで始める」で `true` に。以後は自動では開かない（レール ⚙ からいつでも開ける）。
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "necoder-dark".to_string(),
            density: Density::Compact,
            font_size: 13.0,
            tab_size: 4,
            soft_wrap: false,
            format_on_save: false,
            auto_save: "off".to_string(),
            preview_tabs: true,
            locale: None,
            submit_on_enter: false,
            agent_auto_name: true,
            agent_prewarm: true,
            sound_done: "nyaan".to_string(),
            sound_waiting: "nyaan".to_string(),
            sound_volume: 100,
            system_notifications: true,
            reduce_motion: false,
            tier2_summaries: true,
            captain_agent: None,
            agent_tabs_view: "bar".to_string(),
            work_tabs_position: "top".to_string(),
            default_agent: "Claude Code".to_string(),
            agent_config_defaults: BTreeMap::new(),
            agent_servers: BTreeMap::new(),
            agent_permission_default: "default".to_string(),
            disabled_agents: Vec::new(),
            mcp_servers: BTreeMap::new(),
            confirm_worktree_delete: true,
            confirm_quit: "running".to_string(),
            keep_awake: "working".to_string(),
            ui_font_family: String::new(),
            code_font_family: String::new(),
            terminal_font_size: 12.5,
            terminal_font_family: String::new(),
            terminal_scrollback: 10_000,
            terminal_cursor: "block".to_string(),
            terminal_color_scheme: String::new(),
            statusbar_hidden: Vec::new(),
            terminal_shell: String::new(),
            terminal_shell_args: Vec::new(),
            quick_commands: Vec::new(),
            fleet_agent_worktree: false,
            html_preview_evict_minutes: 15,
            agent_idle_stop_minutes: 15,
            claude_ai_connectors: true,
            allow_terminal_send: false,
            rail: RailSettings::default(),
            chat: ChatSettings::default(),
            fleet_hint_seen: false,
            onboarded: false,
        }
    }
}

/// 同梱している猫の声（`sound_done` / `sound_waiting` に書ける値・設定画面の並び順もこれ）。
/// 実体は `assets/sounds/<声>-<場面>.wav`（`scripts/gen-chime.py` の合成）で、鳴らすのは
/// `agent_panel::sound`。ここに置くのは「設定が受け取れる値」の正が settings 側だから。
pub const SOUND_VOICES: [&str; 3] = ["nyaan", "nya", "mew"];

/// ⌘Q・最後の窓を閉じる時に確認するか（`confirm_quit` の解釈・O4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitConfirmation {
    /// 動いているもの（実行中・承認待ち/質問待ちのエージェント、前面でプロセスが動く端末）が
    /// ある時だけ確認する。何も動いていなければ今までどおり即終了する。
    WhenRunning,
    /// 確認しない。
    Never,
}

/// エージェントの作業中にスリープを止めるか（`keep_awake` の解釈・O13）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAwake {
    /// 作業中（Working）のスレッドが 1 本でもある間だけ、放っておいた時のスリープを止める。
    /// 承認待ち・質問待ちは数えない（人の返事を待つ間まで起こし続けない）。
    WhileWorking,
    /// 止めない（OS の設定どおりに眠る）。
    Off,
}

/// 自動保存の仕方（`auto_save` の解釈・O26）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoSave {
    /// しない（⌘S だけ）。
    Off,
    /// 他へ移った時（エディタからフォーカスが外れた・窓を離れた）に保存する。
    OnFocusChange,
    /// 手を止めて少したった時と、他へ移った時に保存する。
    AfterDelay,
}

impl Settings {
    /// エージェント（`AgentKind::id`）を使うか（`disabled_agents` に無ければ使う・O16）。
    pub fn agent_enabled(&self, agent_id: &str) -> bool {
        !self.disabled_agents.iter().any(|id| id == agent_id)
    }

    /// `auto_save` の値。**知らない値は保存しない側に倒す**（綴り違いで、頼んでいない書き込みを
    /// ディスクへ始めない）。
    pub fn auto_save_mode(&self) -> AutoSave {
        match self.auto_save.as_str() {
            "focus_change" => AutoSave::OnFocusChange,
            "after_delay" => AutoSave::AfterDelay,
            _ => AutoSave::Off,
        }
    }

    /// `keep_awake` の値。**知らない値は止める側に倒す**（綴り違いで作業中のターンが寝て途切れる方が、
    /// 作業の間だけ起きている電力より高くつく）。
    pub fn keep_awake_mode(&self) -> KeepAwake {
        match self.keep_awake.as_str() {
            "off" => KeepAwake::Off,
            _ => KeepAwake::WhileWorking,
        }
    }

    /// `confirm_quit` の値。**知らない値は確認する側に倒す**（綴り違いで黙ってエージェントを
    /// 止める方が、1 回余計に聞かれるより高くつく）。
    pub fn quit_confirmation(&self) -> QuitConfirmation {
        match self.confirm_quit.as_str() {
            "never" => QuitConfirmation::Never,
            _ => QuitConfirmation::WhenRunning,
        }
    }
}

/// 組み込みの既定設定（最下層。ユーザーが見られる正の既定値）。
pub const DEFAULT_SETTINGS_JSON: &str = r#"{
  "theme": "necoder-dark",
  "density": "compact",
  "font_size": 13.0,
  "tab_size": 4,
  "auto_save": "off",
  "preview_tabs": true,
  "submit_on_enter": false,
  "agent_auto_name": true,
  "agent_prewarm": true,
  "sound_done": "nyaan",
  "sound_waiting": "nyaan",
  "sound_volume": 100,
  "system_notifications": true,
  "reduce_motion": false,
  "tier2_summaries": true,
  "agent_tabs_view": "bar",
  "default_agent": "Claude Code",
  "confirm_worktree_delete": true,
  "confirm_quit": "running",
  "keep_awake": "working",
  "ui_font_family": "",
  "code_font_family": "",
  "terminal_font_size": 12.5,
  "terminal_font_family": "",
  "terminal_scrollback": 10000,
  "terminal_cursor": "block",
  "terminal_color_scheme": "",
  "statusbar_hidden": [],
  "terminal_shell": "",
  "terminal_shell_args": [],
  "quick_commands": [],
  "agent_servers": {},
  "agent_permission_default": "default",
  "disabled_agents": [],
  "mcp_servers": {},
  "html_preview_evict_minutes": 15,
  "agent_idle_stop_minutes": 15,
  "claude_ai_connectors": true,
  "allow_terminal_send": false,
  "onboarded": false,
  "rail": { "explorer": true, "search": true, "git": true, "agent": true, "terminal": true, "remote": true },
  "chat": { "directory": "", "instructions": "", "idle_stop_minutes": 10 }
}"#;

/// マージ済み JSON と型付き設定を保持する。
#[derive(Debug, Clone)]
pub struct SettingsStore {
    merged: Value,
    settings: Settings,
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::from_json_layers(&[DEFAULT_SETTINGS_JSON]).unwrap_or(SettingsStore {
            merged: Value::Null,
            settings: Settings::default(),
        })
    }
}

impl SettingsStore {
    /// JSON レイヤ列（後ろほど優先）をマージして解決する。
    pub fn from_json_layers(layers: &[&str]) -> Result<SettingsStore> {
        let mut merged = Value::Object(serde_json::Map::new());
        for (index, layer) in layers.iter().enumerate() {
            let value: Value = serde_json::from_str(layer)
                .with_context(|| format!("設定レイヤ {index} の JSON が不正"))?;
            merge_value(&mut merged, &value);
        }
        let settings: Settings =
            serde_json::from_value(merged.clone()).context("設定のデシリアライズに失敗")?;
        Ok(SettingsStore { merged, settings })
    }

    /// 既定 + user（任意）+ project（`.necoder/settings.json`、任意）を読み込む。
    /// 読めないファイルはスキップ、JSON 破損時は既定で継続（黙って落とさず標準エラーに残す）。
    pub fn load(user_path: Option<&Path>, project_dir: Option<&Path>) -> SettingsStore {
        let mut layers: Vec<String> = vec![DEFAULT_SETTINGS_JSON.to_string()];
        if let Some(path) = user_path {
            if let Ok(text) = std::fs::read_to_string(path) {
                layers.push(text);
            }
        }
        if let Some(dir) = project_dir {
            let path = dir.join(".necoder").join("settings.json");
            if let Ok(text) = std::fs::read_to_string(&path) {
                layers.push(text);
            }
        }
        let refs: Vec<&str> = layers.iter().map(String::as_str).collect();
        SettingsStore::from_json_layers(&refs).unwrap_or_else(|error| {
            eprintln!("設定の読み込みに失敗（既定で継続）: {error:#}");
            SettingsStore::default()
        })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn merged(&self) -> &Value {
        &self.merged
    }
}

/// user 設定ファイルの標準パス。置き場の決定は `paths` crate に集約している（WINDOWS-PORT.md §D1）。
pub fn user_settings_path() -> Option<PathBuf> {
    paths::settings_file()
}

/// 既存の設定ファイルを JSON として読めなかった（手編集の途中の末尾カンマ・コメント・
/// トップレベルがオブジェクトでない等）。**書き手はこの時ファイルに一切触らない** —
/// 空オブジェクトから作り直すと、利用者の設定を 1 キーだけ残して全部消してしまう。
/// UI はこれを見て「読めないので保存しなかった」＋理由を知らせる。
#[derive(Debug)]
pub struct UnreadableSettings {
    pub path: PathBuf,
    /// 読めなかった理由（JSON パーサの行・桁つきの文など）。
    pub reason: String,
}

impl std::fmt::Display for UnreadableSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} を読めないので保存しなかった: {}",
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for UnreadableSettings {}

/// 書き換える前の設定ファイルを読む。無ければ（中身が空白だけでも）空オブジェクト。
/// 在るのに JSON オブジェクトとして読めなければ [`UnreadableSettings`]（呼び手は書かずに返す）。
fn read_settings_object(path: &Path) -> Result<serde_json::Map<String, Value>> {
    let unreadable = |reason: String| UnreadableSettings {
        path: path.to_path_buf(),
        reason,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::Map::new());
        }
        Err(error) => return Err(unreadable(error.to_string()).into()),
    };
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(unreadable("トップレベルが JSON オブジェクトではない".to_string()).into()),
        Err(error) => Err(unreadable(error.to_string()).into()),
    }
}

/// 設定ファイルを pretty JSON で書く。親ディレクトリが無ければ作る。
fn write_settings_object(path: &Path, root: serde_json::Map<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("設定ディレクトリを作れない: {}", parent.display()))?;
    }
    let text =
        serde_json::to_string_pretty(&Value::Object(root)).context("設定の JSON 化に失敗")?;
    std::fs::write(path, text).with_context(|| format!("設定を書けない: {}", path.display()))?;
    Ok(())
}

/// `root[key]` をオブジェクトとして取り出す（無い・オブジェクトでなければ空オブジェクトに置き換える）。
fn object_entry<'a>(
    root: &'a mut serde_json::Map<String, Value>,
    key: &str,
) -> &'a mut serde_json::Map<String, Value> {
    let entry = root
        .entry(key)
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(serde_json::Map::new());
    }
    entry.as_object_mut().expect("直前で object を保証")
}

/// user 設定ファイルの 1 キーだけを書き換えて保存する（アプリ内トグルの永続化用）。
/// 既存 JSON を読んで（無ければ空オブジェクト）、`key` を `value` にして pretty で書き戻す。
/// 他のキー・ユーザーの値は保つ。親ディレクトリが無ければ作る。
/// 既存ファイルを読めなければ**書かずに** [`UnreadableSettings`] を返す（黙って壊さない）。
pub fn persist_user_value(path: &Path, key: &str, value: Value) -> Result<()> {
    let mut root = read_settings_object(path)?;
    root.insert(key.to_string(), value);
    write_settings_object(path, root)
}

/// `<section>.<key>` の 1 点だけを user 設定ファイルへ書き込む（`chat.directory` のような 1 段の入れ子）。
/// `persist_user_value` はトップレベルのキーしか書けない（`"chat.directory"` という名前のキーになる）。
/// **user ファイル自身の値だけ**を読んで更新する（マージ済みの解決値を書き戻すと project 層を焼き込む）。
/// 同じ section の他のキーは保つ。
pub fn persist_nested_value(path: &Path, section: &str, key: &str, value: Value) -> Result<()> {
    let mut root = read_settings_object(path)?;
    object_entry(&mut root, section).insert(key.to_string(), value);
    write_settings_object(path, root)
}

/// `agent_config_defaults.<agent_id>.<config_id>` の 1 点だけを user 設定ファイルへ書き込む（ピルの sticky 保存用）。
/// **user ファイル自身の値だけ**を読んで nested に更新する（マージ済み解決値を書き戻すと project 層の値を
/// user へ焼き込んでしまうため）。既存の他 agent・他 field・他キーは保つ。親ディレクトリが無ければ作る。
pub fn persist_agent_config_default(
    path: &Path,
    agent_id: &str,
    config_id: &str,
    value_id: &str,
) -> Result<()> {
    let mut root = read_settings_object(path)?;
    let defaults = object_entry(&mut root, "agent_config_defaults");
    object_entry(defaults, agent_id)
        .insert(config_id.to_string(), Value::String(value_id.to_string()));
    write_settings_object(path, root)
}

/// `mcp_servers.<name>.enabled` の 1 点だけを user 設定ファイルへ書き込む（設定画面のトグル）。
/// [`persist_agent_config_default`] と同じ理由で **user ファイル自身の値だけ**を読んで更新する
/// （マージ済みの解決値を書き戻すと project 層の定義を user へ焼き込んでしまう）。
/// 自前定義（`command` / `url` を持つ行）の他フィールドは触らない。
pub fn persist_mcp_enabled(path: &Path, name: &str, enabled: bool) -> Result<()> {
    let mut root = read_settings_object(path)?;
    let servers = object_entry(&mut root, "mcp_servers");
    object_entry(servers, name).insert("enabled".to_string(), Value::Bool(enabled));
    write_settings_object(path, root)
}

/// `agent_config_defaults.<agent_id>.<config_id>` の 1 点を user 設定ファイルから消す（ピルの記憶を
/// 忘れる・O16）。消した結果その agent の記憶が空になれば agent ごと消す（`{}` を残さない）。
pub fn forget_agent_config_default(path: &Path, agent_id: &str, config_id: &str) -> Result<()> {
    let mut root = read_settings_object(path)?;
    let defaults = object_entry(&mut root, "agent_config_defaults");
    let now_empty = match defaults.get_mut(agent_id).and_then(Value::as_object_mut) {
        Some(agent) => {
            agent.remove(config_id);
            agent.is_empty()
        }
        None => false,
    };
    if now_empty {
        defaults.remove(agent_id);
    }
    write_settings_object(path, root)
}

/// 権限モードの既定（`agent_permission_default`）を書き、全エージェントのピルの記憶（`mode`）を消す
/// （一括で揃える・O16）。モデルや思考量の記憶は触らない。1 回の書き込みで済ませる。
pub fn persist_permission_default(path: &Path, value: &str) -> Result<()> {
    let mut root = read_settings_object(path)?;
    root.insert(
        "agent_permission_default".to_string(),
        Value::String(value.to_string()),
    );
    let defaults = object_entry(&mut root, "agent_config_defaults");
    for agent in defaults.values_mut() {
        if let Some(agent) = agent.as_object_mut() {
            agent.remove("mode");
        }
    }
    defaults.retain(|_, agent| agent.as_object().is_none_or(|agent| !agent.is_empty()));
    write_settings_object(path, root)
}

/// `agent_servers.<agent_id>.env.<var>` の 1 点だけを user 設定ファイルへ書く（`None` = 消す・O14 の
/// アカウント切替）。起動方法（`type` / `command` / `args`）と他の環境変数は触らない。項目が無ければ
/// `{"type": "registry"}` として作る。消した結果 `registry` で env も空になったら、その項目ごと消す
/// （`{}` を残さない）。[`persist_agent_config_default`] と同じく user ファイル自身の値だけを読む。
pub fn persist_agent_server_env(
    path: &Path,
    agent_id: &str,
    var: &str,
    value: Option<&str>,
) -> Result<()> {
    let mut root = read_settings_object(path)?;
    let servers = object_entry(&mut root, "agent_servers");
    let remove_entry = {
        let server = object_entry(servers, agent_id);
        server
            .entry("type")
            .or_insert_with(|| Value::String("registry".to_string()));
        let env = object_entry(server, "env");
        match value {
            Some(value) => {
                env.insert(var.to_string(), Value::String(value.to_string()));
            }
            None => {
                env.remove(var);
            }
        }
        let env_empty = env.is_empty();
        if env_empty {
            server.remove("env");
        }
        env_empty
            && server.len() == 1
            && server.get("type").and_then(Value::as_str) == Some("registry")
    };
    if remove_entry {
        servers.remove(agent_id);
    }
    write_settings_object(path, root)
}

/// `agent_servers.<agent_id>` を丸ごと書き換える（`None` = 消す＝レジストリと組み込みの既定の起動に
/// 戻す・O16 の起動の上書きの画面）。**user ファイル自身の値だけ**を読んで書く。
pub fn persist_agent_server(
    path: &Path,
    agent_id: &str,
    setting: Option<&AgentServerSetting>,
) -> Result<()> {
    let mut root = read_settings_object(path)?;
    let servers = object_entry(&mut root, "agent_servers");
    match setting {
        Some(setting) => {
            servers.insert(agent_id.to_string(), setting.to_json());
        }
        None => {
            servers.remove(agent_id);
        }
    }
    write_settings_object(path, root)
}

/// アカウント切替（O14）に使う環境変数 = そのエージェントの設定の置き場を変える物。対応していない
/// エージェントは `None`。資格情報そのものは necoder が読みも写しもしない（置き場を指すだけ）。
pub fn account_env_var(agent_id: &str) -> Option<&'static str> {
    match agent_id {
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        "codex" => Some("CODEX_HOME"),
        _ => None,
    }
}

/// necoder が作るアカウントのフォルダの置き場（`<data>/accounts/<agent_id>`）。
pub fn accounts_root(agent_id: &str) -> Option<PathBuf> {
    paths::data_dir().map(|dir| dir.join("accounts").join(agent_id))
}

/// 置き場にあるアカウント（フォルダ名の昇順・隠しフォルダは除く）。置き場が無ければ空。
pub fn list_accounts(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// 次に作るアカウントの名前（`account-2` から・既定のアカウントを 1 本目と数える）。
pub fn next_account_name(existing: &[String]) -> String {
    (2..)
        .map(|number| format!("account-{number}"))
        .find(|name| !existing.contains(name))
        .unwrap_or_else(|| "account".to_string())
}

/// `overlay` を `base` に深くマージする。オブジェクトは再帰、それ以外は置換。
fn merge_value(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                match base_map.get_mut(key) {
                    Some(base_value) => merge_value(base_value, overlay_value),
                    None => {
                        base_map.insert(key.clone(), overlay_value.clone());
                    }
                }
            }
        }
        (base_slot, overlay_value) => *base_slot = overlay_value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_value_is_written_without_disturbing_its_neighbours() {
        let path = std::env::temp_dir().join(format!(
            "necoder_settings_nested_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(
            &path,
            r#"{"font_size": 15, "chat": {"instructions": "敬語は使わない"}}"#,
        )
        .unwrap();
        persist_nested_value(&path, "chat", "directory", Value::String("~/Chats".into())).unwrap();
        persist_nested_value(&path, "chat", "idle_stop_minutes", serde_json::json!(5)).unwrap();

        let store = SettingsStore::load(Some(&path), None);
        let settings = store.settings();
        assert_eq!(settings.chat.directory, "~/Chats");
        assert_eq!(settings.chat.idle_stop_minutes, 5);
        assert_eq!(
            settings.chat.instructions, "敬語は使わない",
            "同じ section の他の値は保つ"
        );
        assert_eq!(settings.font_size, 15.0, "他のキーも保つ");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn default_layer_resolves_to_defaults() {
        let store = SettingsStore::default();
        assert_eq!(store.settings().theme, "necoder-dark");
        assert_eq!(store.settings().density, Density::Compact);
        assert_eq!(store.settings().tab_size, 4);
    }

    #[test]
    fn user_layer_overrides_default() {
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "theme": "necoder-light", "tab_size": 2 }"#,
        ])
        .expect("マージできる");
        assert_eq!(store.settings().theme, "necoder-light");
        assert_eq!(store.settings().tab_size, 2);
        // 触れていないキーは既定のまま
        assert_eq!(store.settings().density, Density::Compact);
    }

    #[test]
    fn project_layer_overrides_user() {
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "theme": "necoder-light", "density": "cozy" }"#, // user
            r#"{ "theme": "necoder-dark" }"#,                     // project が最優先
        ])
        .expect("マージできる");
        assert_eq!(store.settings().theme, "necoder-dark"); // project 勝ち
        assert_eq!(store.settings().density, Density::Cozy); // user のまま
    }

    #[test]
    fn merge_is_deep_for_nested_objects() {
        let mut base: Value = serde_json::from_str(r#"{ "a": { "x": 1, "y": 2 } }"#).unwrap();
        let overlay: Value = serde_json::from_str(r#"{ "a": { "y": 9, "z": 3 } }"#).unwrap();
        merge_value(&mut base, &overlay);
        assert_eq!(
            base,
            serde_json::from_str::<Value>(r#"{ "a": { "x": 1, "y": 9, "z": 3 } }"#).unwrap()
        );
    }

    #[test]
    fn reads_both_shapes_of_mcp_server_settings() {
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "mcp_servers": {
                   "higgsfield": { "enabled": true },
                   "tools": { "command": "npx", "args": ["-y", "tools-mcp"], "env": { "A": "1" } },
                   "private": { "type": "http", "url": "https://example.invalid/mcp",
                                "headers": { "Authorization": "Bearer ${TOKEN}" } }
                 } }"#,
        ])
        .expect("マージできる");
        let servers = &store.settings().mcp_servers;
        assert_eq!(servers.len(), 3);
        // 有効/無効だけの行（発見済みサーバのトグル）。
        let higgsfield = &servers["higgsfield"];
        assert_eq!(higgsfield.enabled, Some(true));
        assert!(higgsfield.command.is_none() && higgsfield.url.is_none());
        // 自前定義（stdio）。
        assert_eq!(servers["tools"].command.as_deref(), Some("npx"));
        assert_eq!(servers["tools"].args, vec!["-y", "tools-mcp"]);
        assert_eq!(servers["tools"].env["A"], "1");
        // 自前定義（http）。`type` は `transport` の別名として読める。
        assert_eq!(servers["private"].transport.as_deref(), Some("http"));
        assert_eq!(
            servers["private"].headers["Authorization"],
            "Bearer ${TOKEN}"
        );
    }

    #[test]
    fn persists_only_the_enabled_flag_of_one_mcp_server() {
        let dir = std::env::temp_dir().join(format!("necoder_mcp_persist_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).expect("作れる");
        std::fs::write(
            &path,
            r#"{ "theme": "necoder-light",
                 "mcp_servers": { "tools": { "command": "npx", "enabled": true } } }"#,
        )
        .expect("書ける");

        persist_mcp_enabled(&path, "tools", false).expect("保存できる");
        persist_mcp_enabled(&path, "higgsfield", true).expect("保存できる");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("読める")).expect("JSON");
        // 他のキーも、自前定義の他フィールドも触らない。
        assert_eq!(written["theme"], "necoder-light");
        assert_eq!(written["mcp_servers"]["tools"]["command"], "npx");
        assert_eq!(written["mcp_servers"]["tools"]["enabled"], false);
        // 未知の名前は「発見済みサーバの on/off だけの行」として足される。
        assert_eq!(written["mcp_servers"]["higgsfield"]["enabled"], true);
        assert!(written["mcp_servers"]["higgsfield"]["command"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn density_line_height() {
        assert_eq!(Density::Compact.line_height(), 23.0);
        assert_eq!(Density::Cozy.line_height(), 27.0);
    }

    #[test]
    fn invalid_layer_reports_error() {
        assert!(SettingsStore::from_json_layers(&["{ not json"]).is_err());
    }

    #[test]
    fn submit_on_enter_defaults_off_and_overrides() {
        assert!(!SettingsStore::default().settings().submit_on_enter);
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "submit_on_enter": true }"#,
        ])
        .expect("マージできる");
        assert!(store.settings().submit_on_enter);
    }

    #[test]
    fn quit_confirmation_defaults_to_when_running_and_can_be_turned_off() {
        assert_eq!(
            SettingsStore::default().settings().quit_confirmation(),
            QuitConfirmation::WhenRunning
        );
        let never = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "confirm_quit": "never" }"#,
        ])
        .expect("マージできる");
        assert_eq!(
            never.settings().quit_confirmation(),
            QuitConfirmation::Never
        );
        // 綴り違いは黙って「聞かない」にしない。
        let typo = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "confirm_quit": "nevr" }"#,
        ])
        .expect("マージできる");
        assert_eq!(
            typo.settings().quit_confirmation(),
            QuitConfirmation::WhenRunning
        );
    }

    #[test]
    fn account_env_is_written_without_touching_the_rest() {
        let path =
            std::env::temp_dir().join(format!("necoder_agent_env_{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"theme":"x","agent_servers":{"codex":{"type":"custom","command":"codex-acp","env":{"KEEP":"1"}}}}"#,
        )
        .expect("書ける");
        persist_agent_server_env(&path, "claude", "CLAUDE_CONFIG_DIR", Some("/a/work"))
            .expect("書ける");
        persist_agent_server_env(&path, "codex", "CODEX_HOME", Some("/a/codex")).expect("書ける");
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            &std::fs::read_to_string(&path).expect("読める"),
        ])
        .expect("読める");
        let servers = &store.settings().agent_servers;
        assert_eq!(
            servers["claude"]
                .env()
                .get("CLAUDE_CONFIG_DIR")
                .map(String::as_str),
            Some("/a/work")
        );
        assert!(
            matches!(servers["codex"], AgentServerSetting::Custom { .. }),
            "起動方法は変えない"
        );
        assert_eq!(
            servers["codex"].env().get("KEEP").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            servers["codex"].env().get("CODEX_HOME").map(String::as_str),
            Some("/a/codex")
        );

        // 既定へ戻す = 消す。registry だけの項目は丸ごと消え、custom は残る。
        persist_agent_server_env(&path, "claude", "CLAUDE_CONFIG_DIR", None).expect("書ける");
        persist_agent_server_env(&path, "codex", "CODEX_HOME", None).expect("書ける");
        let text = std::fs::read_to_string(&path).expect("読める");
        let value: Value = serde_json::from_str(&text).expect("JSON");
        assert!(value["agent_servers"].get("claude").is_none(), "{text}");
        assert_eq!(value["agent_servers"]["codex"]["command"], "codex-acp");
        assert_eq!(value["theme"], "x");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn account_names_count_from_two() {
        assert_eq!(next_account_name(&[]), "account-2");
        assert_eq!(
            next_account_name(&["account-2".to_string(), "work".to_string()]),
            "account-3"
        );
        assert_eq!(account_env_var("claude"), Some("CLAUDE_CONFIG_DIR"));
        assert_eq!(account_env_var("codex"), Some("CODEX_HOME"));
        assert_eq!(account_env_var("opencode"), None);
    }

    #[test]
    fn auto_save_defaults_off_and_unknown_values_do_not_save() {
        assert_eq!(
            SettingsStore::default().settings().auto_save_mode(),
            AutoSave::Off
        );
        for (value, expected) in [
            ("focus_change", AutoSave::OnFocusChange),
            ("after_delay", AutoSave::AfterDelay),
            ("off", AutoSave::Off),
            ("afterDelay", AutoSave::Off),
        ] {
            let layer = format!(r#"{{ "auto_save": "{value}" }}"#);
            let store = SettingsStore::from_json_layers(&[DEFAULT_SETTINGS_JSON, &layer])
                .expect("マージできる");
            assert_eq!(store.settings().auto_save_mode(), expected, "{value}");
        }
    }

    #[test]
    fn keep_awake_defaults_to_while_working_and_can_be_turned_off() {
        assert_eq!(
            SettingsStore::default().settings().keep_awake_mode(),
            KeepAwake::WhileWorking
        );
        let off =
            SettingsStore::from_json_layers(&[DEFAULT_SETTINGS_JSON, r#"{ "keep_awake": "off" }"#])
                .expect("マージできる");
        assert_eq!(off.settings().keep_awake_mode(), KeepAwake::Off);
        // 綴り違いで黙って「止めない」にしない。
        let typo =
            SettingsStore::from_json_layers(&[DEFAULT_SETTINGS_JSON, r#"{ "keep_awake": "of" }"#])
                .expect("マージできる");
        assert_eq!(typo.settings().keep_awake_mode(), KeepAwake::WhileWorking);
    }

    /// O25: ターミナルの見た目は既定で設定を持つ前と同じ（12.5pt・1 万行・ブロック）。書けば上書き。
    #[test]
    fn terminal_appearance_defaults_to_the_old_look_and_overrides() {
        let defaults = SettingsStore::default();
        let settings = defaults.settings();
        assert_eq!(settings.terminal_font_size, 12.5);
        assert_eq!(settings.terminal_font_family, "");
        assert_eq!(settings.terminal_scrollback, 10_000);
        assert_eq!(settings.terminal_cursor, "block");
        assert_eq!(settings.terminal_shell, "", "空 = OS の既定のシェル");
        assert_eq!(settings.ui_font_family, "", "空 = 同梱の書体");
        assert_eq!(settings.code_font_family, "");
        assert!(settings.terminal_shell_args.is_empty());
        assert!(settings.quick_commands.is_empty());
        assert_eq!(
            *settings,
            Settings::default(),
            "既定の JSON と型の既定が同じ"
        );
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "terminal_font_size": 15, "terminal_font_family": "Menlo", "terminal_scrollback": 50000, "terminal_cursor": "bar" }"#,
        ])
        .expect("マージできる");
        let settings = store.settings();
        assert_eq!(settings.terminal_font_size, 15.0, "整数で書いても読める");
        assert_eq!(settings.terminal_font_family, "Menlo");
        assert_eq!(settings.terminal_scrollback, 50_000);
        assert_eq!(settings.terminal_cursor, "bar");
    }

    #[test]
    fn reduce_motion_defaults_off_and_overrides() {
        assert!(!SettingsStore::default().settings().reduce_motion);
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "reduce_motion": true }"#,
        ])
        .expect("マージできる");
        assert!(store.settings().reduce_motion);
    }

    #[test]
    fn persist_user_value_sets_one_key_and_keeps_others() {
        let dir =
            std::env::temp_dir().join(format!("necoder-settings-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        // 既存にユーザー値がある状態を作る
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(&path, r#"{ "theme": "necoder-light" }"#).expect("seed");

        persist_user_value(&path, "submit_on_enter", Value::Bool(true)).expect("書ける");
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            &std::fs::read_to_string(&path).expect("read"),
        ])
        .expect("マージできる");
        assert!(store.settings().submit_on_enter); // 書いたキー
        assert_eq!(store.settings().theme, "necoder-light"); // 既存キーは保たれる
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persist_agent_config_default_is_nested_and_isolated() {
        let dir = std::env::temp_dir().join(format!(
            "necoder-agent-defaults-test-{}",
            std::process::id()
        ));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(&path, r#"{ "theme": "necoder-light" }"#).expect("seed");

        // 別 agent・別 config_id を順に書いても、互いを潰さず nested にマージされる。
        // 値は ACP が広告する value_id そのまま（`opus[1m]` の角括弧も素通しで往復する）。
        persist_agent_config_default(&path, "claude", "model", "opus[1m]").expect("書ける");
        persist_agent_config_default(&path, "claude", "effort", "xhigh").expect("書ける");
        persist_agent_config_default(&path, "codex", "model", "gpt-5.6-sol").expect("書ける");
        // necoder が UI を持たない config_id も素通しで保存できる（Zed 流の汎用マップ）。
        persist_agent_config_default(&path, "claude", "fast", "on").expect("書ける");

        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            &std::fs::read_to_string(&path).expect("read"),
        ])
        .expect("マージできる");
        let defaults = &store.settings().agent_config_defaults;
        assert_eq!(defaults["claude"]["model"], "opus[1m]");
        assert_eq!(defaults["claude"]["effort"], "xhigh");
        assert_eq!(defaults["claude"]["fast"], "on");
        assert_eq!(defaults["codex"]["model"], "gpt-5.6-sol");
        assert!(!defaults["codex"].contains_key("effort")); // 書いていない config_id は不在
        assert_eq!(store.settings().theme, "necoder-light"); // 無関係キーは保たれる
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 手編集の途中で壊れている settings.json（末尾カンマ・コメント・配列）に書き手が触ると、
    /// 以前は空オブジェクトから作り直して 1 キーだけで上書きしていた（利用者の設定が全部消える）。
    /// 今は**どの書き手も書かずに** `UnreadableSettings` を返し、ファイルは 1 バイトも変わらない。

    /// O16: 起動の上書きの欄は 1 行 1 つ。誤りは行番号つきで断り、書くと読める形になる。
    #[test]
    fn launch_override_fields_round_trip() {
        let env = parse_env_lines("# メモ\nCODEX_HOME = /opt/codex\n\nRUST_LOG=debug=1\n")
            .expect("読める");
        assert_eq!(env["CODEX_HOME"], "/opt/codex");
        assert_eq!(env["RUST_LOG"], "debug=1", "値の中の = はそのまま");
        assert_eq!(env_lines(&env), "CODEX_HOME=/opt/codex\nRUST_LOG=debug=1");
        assert!(parse_env_lines("NO_EQUALS")
            .unwrap_err()
            .starts_with("1 行目"));
        assert!(parse_env_lines("ok=1\n9BAD=x")
            .unwrap_err()
            .starts_with("2 行目"));
        assert_eq!(
            parse_arg_lines("  --acp \n\n--model gpt 5\n"),
            vec!["--acp".to_string(), "--model gpt 5".to_string()]
        );

        let dir = std::env::temp_dir().join(format!("necoder-agent-server-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{ "theme": "necoder-light" }"#).expect("seed");
        let custom = AgentServerSetting::Custom {
            command: "codex-acp".into(),
            args: vec!["--verbose".into()],
            env,
        };
        persist_agent_server(&path, "codex", Some(&custom)).expect("書ける");
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            &std::fs::read_to_string(&path).expect("read"),
        ])
        .expect("マージできる");
        assert_eq!(store.settings().agent_servers.get("codex"), Some(&custom));
        persist_agent_server(&path, "codex", None).expect("消せる");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("JSON");
        assert!(written["agent_servers"].get("codex").is_none());
        assert_eq!(written["theme"], "necoder-light");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// O16: 権限の既定を選ぶと、全エージェントのピルの記憶（mode）だけを消す。1 つだけ忘れることもできる。
    #[test]
    fn permission_default_clears_remembered_modes_only() {
        let dir = std::env::temp_dir().join(format!(
            "necoder-permission-default-test-{}",
            std::process::id()
        ));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            &path,
            r#"{ "agent_config_defaults": {
                "claude": { "mode": "plan", "model": "opus" },
                "qwen": { "mode": "yolo" } } }"#,
        )
        .expect("seed");

        persist_permission_default(&path, "bypass").expect("書ける");
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            &std::fs::read_to_string(&path).expect("read"),
        ])
        .expect("マージできる");
        let settings = store.settings();
        assert_eq!(settings.agent_permission_default, "bypass");
        let defaults = &settings.agent_config_defaults;
        assert_eq!(
            defaults["claude"].get("model").map(String::as_str),
            Some("opus"),
            "モデルの記憶は残す"
        );
        assert!(defaults["claude"].get("mode").is_none());
        assert!(!defaults.contains_key("qwen"), "空になった agent は消す");

        persist_agent_config_default(&path, "claude", "mode", "plan").expect("書ける");
        forget_agent_config_default(&path, "claude", "model").expect("書ける");
        forget_agent_config_default(&path, "claude", "mode").expect("書ける");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("JSON");
        assert!(
            written["agent_config_defaults"].get("claude").is_none(),
            "{written}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn writers_refuse_to_overwrite_an_unreadable_settings_file() {
        let dir = std::env::temp_dir().join(format!(
            "necoder-settings-unreadable-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        let broken = [
            "{\n  \"theme\": \"necoder-light\",\n  \"font_size\": 15,\n}\n",
            "{\n  // 手で書いたメモ\n  \"theme\": \"necoder-light\"\n}\n",
            "[\"theme\"]\n",
        ];
        type Writer = fn(&Path) -> Result<()>;
        let writers: [(&str, Writer); 7] = [
            ("persist_user_value", |path| {
                persist_user_value(path, "submit_on_enter", Value::Bool(true))
            }),
            ("persist_nested_value", |path| {
                persist_nested_value(path, "chat", "directory", Value::String("~/Chats".into()))
            }),
            ("persist_agent_config_default", |path| {
                persist_agent_config_default(path, "claude", "model", "opus")
            }),
            ("persist_mcp_enabled", |path| {
                persist_mcp_enabled(path, "tools", true)
            }),
            ("forget_agent_config_default", |path| {
                forget_agent_config_default(path, "claude", "mode")
            }),
            ("persist_permission_default", |path| {
                persist_permission_default(path, "bypass")
            }),
            ("persist_agent_server", |path| {
                persist_agent_server(path, "codex", None)
            }),
        ];
        for original in broken {
            for (name, write) in writers {
                std::fs::write(&path, original).expect("seed");
                let error = write(&path).expect_err(name);
                assert!(
                    error.downcast_ref::<UnreadableSettings>().is_some(),
                    "{name}: 読めない理由として返す: {error:#}"
                );
                assert_eq!(
                    std::fs::read_to_string(&path).expect("read"),
                    original,
                    "{name}: 読めないファイルを書き換えた"
                );
            }
        }

        // 無い・空白だけのファイルは失うものが無いので、そのまま書く。
        std::fs::remove_file(&path).expect("rm");
        persist_user_value(&path, "theme", Value::String("necoder-light".into())).expect("新規");
        std::fs::write(&path, "  \n").expect("seed");
        persist_user_value(&path, "tab_size", serde_json::json!(2)).expect("空白だけ");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("JSON");
        assert_eq!(written["tab_size"], 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
