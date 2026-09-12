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
    /// ターン完了の通知音。`"nya"`（同梱・既定）/ `"system"`（OS の音）/ `"off"` /
    /// 任意のファイルパス（`~/` 可）。裏の窓で走らせた作業の完了に気づくための音
    /// （`docs/BACKGROUND.md` の原点痛点）。**見ている画面では鳴らさない**。
    pub sound_done: String,
    /// 入力待ち（承認・質問で止まった）の通知音。値の取り方は [`Settings::sound_done`] と同じ。
    /// 完了とは違う音を当てて、耳だけで「終わった」と「呼ばれている」を区別する。
    pub sound_waiting: String,
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
    /// 編隊の目標文（管制ヘッダに常時表示・P3）。プロジェクト設定 `.necoder/settings.json` に
    /// 書けばリポジトリごとの目標になる（ファイルが真実の原則＝計画の「ledger」は settings で満たす）。
    pub fleet_goal: Option<String>,
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
    /// スレッドのセッションでエージェントへ渡す MCP サーバ（サーバ名がキー）。詳細は [`McpServerSetting`]。
    /// 空＝他ツールから発見した分だけが一覧に並び、どれも渡さない（有効化は明示だけ）。
    pub mcp_servers: BTreeMap<String, McpServerSetting>,
    /// worktree 削除の前に確認ダイアログを出すか（既定 on・2026-07-27）。
    /// **off にしても「失うものがある」ときは必ず確認する** — 未コミットの変更は git にも残らないので、
    /// 「二度と聞くな」の対象は *取り返しがつく* 削除に限る（DECISIONS の該当項）。
    pub confirm_worktree_delete: bool,
    /// 旧 Fleet の互換設定。TaskSpace-first 以降は既定操作が常に `+ Task` なので挙動には使わない。
    /// 既存 settings.json を壊さず読めるよう schema field だけ保持する。
    pub fleet_agent_worktree: bool,
    /// HTML プレビュー（OS 標準 WebView）を非表示のまま放置したとき、自動破棄するまでの分数（既定 15・
    /// `0` = 自動破棄しない）。WebView は生きている間 数十〜数百 MB を別プロセスで握るため、
    /// idle メモリ予算を守る回収弁。破棄後の再表示は遅延再生成（初回表示と同じ経路）なので、
    /// ローカル HTML では失うものは実質スクロール位置だけ。
    pub html_preview_evict_minutes: u64,
    /// レールのアイコン表示（アクティビティバー）。
    pub rail: RailSettings,
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
            locale: None,
            submit_on_enter: false,
            agent_auto_name: true,
            agent_prewarm: true,
            sound_done: "nya".to_string(),
            sound_waiting: "nya".to_string(),
            reduce_motion: false,
            tier2_summaries: true,
            captain_agent: None,
            fleet_goal: None,
            agent_tabs_view: "bar".to_string(),
            work_tabs_position: "top".to_string(),
            default_agent: "Claude Code".to_string(),
            agent_config_defaults: BTreeMap::new(),
            agent_servers: BTreeMap::new(),
            mcp_servers: BTreeMap::new(),
            confirm_worktree_delete: true,
            fleet_agent_worktree: false,
            html_preview_evict_minutes: 15,
            rail: RailSettings::default(),
            fleet_hint_seen: false,
            onboarded: false,
        }
    }
}

/// 組み込みの既定設定（最下層。ユーザーが見られる正の既定値）。
pub const DEFAULT_SETTINGS_JSON: &str = r#"{
  "theme": "necoder-dark",
  "density": "compact",
  "font_size": 13.0,
  "tab_size": 4,
  "submit_on_enter": false,
  "agent_auto_name": true,
  "agent_prewarm": true,
  "sound_done": "nya",
  "sound_waiting": "nya",
  "reduce_motion": false,
  "tier2_summaries": true,
  "agent_tabs_view": "bar",
  "default_agent": "Claude Code",
  "confirm_worktree_delete": true,
  "agent_servers": {},
  "mcp_servers": {},
  "html_preview_evict_minutes": 15,
  "onboarded": false,
  "rail": { "explorer": true, "search": true, "git": true, "agent": true, "terminal": true, "remote": true }
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

/// user 設定ファイルの 1 キーだけを書き換えて保存する（アプリ内トグルの永続化用）。
/// 既存 JSON を読んで（無ければ空オブジェクト）、`key` を `value` にして pretty で書き戻す。
/// 他のキー・ユーザーの値は保つ。親ディレクトリが無ければ作る。
pub fn persist_user_value(path: &Path, key: &str, value: Value) -> Result<()> {
    let mut root: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    // 破損・非オブジェクトだった場合も空オブジェクトから作り直す（黙って壊さない）。
    if !root.is_object() {
        root = Value::Object(serde_json::Map::new());
    }
    if let Value::Object(map) = &mut root {
        map.insert(key.to_string(), value);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("設定ディレクトリを作れない: {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&root).context("設定の JSON 化に失敗")?;
    std::fs::write(path, text).with_context(|| format!("設定を書けない: {}", path.display()))?;
    Ok(())
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
    let mut root: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let map = root.as_object_mut().expect("上で object を保証");
    let defaults = map
        .entry("agent_config_defaults")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !defaults.is_object() {
        *defaults = Value::Object(serde_json::Map::new());
    }
    let agents = defaults.as_object_mut().expect("直前で object を保証");
    let entry = agents
        .entry(agent_id)
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(serde_json::Map::new());
    }
    entry
        .as_object_mut()
        .expect("直前で object を保証")
        .insert(config_id.to_string(), Value::String(value_id.to_string()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("設定ディレクトリを作れない: {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&root).context("設定の JSON 化に失敗")?;
    std::fs::write(path, text).with_context(|| format!("設定を書けない: {}", path.display()))?;
    Ok(())
}

/// `mcp_servers.<name>.enabled` の 1 点だけを user 設定ファイルへ書き込む（設定画面のトグル）。
/// [`persist_agent_config_default`] と同じ理由で **user ファイル自身の値だけ**を読んで更新する
/// （マージ済みの解決値を書き戻すと project 層の定義を user へ焼き込んでしまう）。
/// 自前定義（`command` / `url` を持つ行）の他フィールドは触らない。
pub fn persist_mcp_enabled(path: &Path, name: &str, enabled: bool) -> Result<()> {
    let mut root: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let map = root.as_object_mut().expect("上で object を保証");
    let servers = map
        .entry("mcp_servers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !servers.is_object() {
        *servers = Value::Object(serde_json::Map::new());
    }
    let entry = servers
        .as_object_mut()
        .expect("直前で object を保証")
        .entry(name)
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(serde_json::Map::new());
    }
    entry
        .as_object_mut()
        .expect("直前で object を保証")
        .insert("enabled".to_string(), Value::Bool(enabled));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("設定ディレクトリを作れない: {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&root).context("設定の JSON 化に失敗")?;
    std::fs::write(path, text).with_context(|| format!("設定を書けない: {}", path.display()))?;
    Ok(())
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
}
