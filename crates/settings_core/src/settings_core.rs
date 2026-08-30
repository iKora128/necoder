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
    /// 編隊（herd サイドバー・状態一覧・M14）。
    pub herd: bool,
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
            herd: true,
        }
    }
}

/// エージェントごとの sticky 既定（モデル / 思考量 / 権限モード）。
/// **どのエージェントを使うか**（`default_agent`・§8）とは別レイヤ — こちらは「作業のたびに選び直したくない」
/// 設定を **agent ごとに** 覚える（Claude を Opus、Codex を GPT-5 のように混ぜても各々が保たれる）。
/// composer のピルで選ぶとここへ書き戻り、その agent の次スレッドが同じ値で開く（2026-08-17）。
/// 各値は `None`＝未選択（フォールバックへ委ねる）。JSON では欠けたキーが `None` になる。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AgentDefaults {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
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
    /// エージェントのターン完了時に通知音を鳴らすか（macOS system sound・既定 on）。
    /// 裏の窓で走らせた作業の完了にも気づける（`docs/BACKGROUND.md` の原点痛点）。独自チャイム同梱は後続。
    pub completion_sound: bool,
    /// 装飾的な動きを静止するアクセシビリティ設定。GPUI の `reduce_motion` へ接続し、
    /// スピナー・fade・マスコットなどの継続アニメーションを静止画として描く。
    pub reduce_motion: bool,
    /// Tier 2 遷移スナップショット（✳ 1 行要約・FLEET-CONTROL-PLAN P4・既定 on）。
    /// Done/Failed 遷移時に既定 Agent の oneshot CLI で 1 行生成する。オフでも Tier 1（決定論）は出続ける。
    pub tier2_summaries: bool,
    /// 監督（coordinator）に任命するエージェント表示名（P6・None = 未任命）。
    /// 任命は settings.json の明示編集（既定ドリフト禁止の原則・DECISIONS §8）。
    /// 任命すると Blocked(15s)/Done/Failed 遷移で IntegrationSpace の「監督」スレッドが 1 ターン起きる。
    pub coordinator_agent: Option<String>,
    /// 編隊の目標文（管制ヘッダに常時表示・P3）。プロジェクト設定 `.necoder/settings.json` に
    /// 書けばリポジトリごとの目標になる（ファイルが真実の原則＝計画の「ledger」は settings で満たす）。
    pub fleet_goal: Option<String>,
    /// スレッドタブの見せ方（"bar" 横タブ / "list" 縦リスト）。Agent パネルのスイッチャがここへ保存し、
    /// 次の起動でも保つ。設定画面のトグル化は後続（真実はこの値・画面はこれを操作するだけ）。
    pub agent_tabs_view: String,
    /// 新規スレッドの既定 AI エージェント（表示名。`acp_client::AGENT_LABELS` のいずれか）。
    /// **変更は Settings 画面（★ 既定にする）でのみ** — composer のピルはこのグローバル既定を書き換えない
    /// （哲学「自分で決めた既定はドリフトしない」・DECISIONS §8）。
    pub default_agent: String,
    /// 新規スレッドの既定モデル / 思考量。**composer のピルで選ぶとここへ書き戻る**（2026-07-27）。
    /// `default_agent` と違い意図的に sticky にしてある — どのエージェントを使うかは「環境の選択」で
    /// 滅多に変えないが、モデル/思考量は「作業のたびに選び直したくない設定」だから（ユーザー要望）。
    /// エージェントが実選択肢を広告していればそちらが優先され、ここは候補が無いときの土台になる。
    pub default_model: String,
    pub default_effort: String,
    /// エージェントごとの sticky 既定（表示名 → モデル/思考量/モード）。**ピルで選ぶとここへ書き戻る**。
    /// `default_model`/`default_effort`（グローバルな土台）より優先し、agent を跨いでも各々を保つ（2026-08-17）。
    /// `default_agent` は §8 のまま Settings 画面だけが変える — 「どの agent か」と「その agent の設定」を分離する。
    pub agent_defaults: BTreeMap<String, AgentDefaults>,
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
            completion_sound: true,
            reduce_motion: false,
            tier2_summaries: true,
            coordinator_agent: None,
            fleet_goal: None,
            agent_tabs_view: "bar".to_string(),
            default_agent: "Claude Code".to_string(),
            default_model: "claude-opus-5".to_string(),
            default_effort: "high".to_string(),
            agent_defaults: BTreeMap::new(),
            confirm_worktree_delete: true,
            fleet_agent_worktree: false,
            html_preview_evict_minutes: 15,
            rail: RailSettings::default(),
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
  "completion_sound": true,
  "reduce_motion": false,
  "tier2_summaries": true,
  "agent_tabs_view": "bar",
  "default_agent": "Claude Code",
  "default_model": "claude-opus-5",
  "default_effort": "high",
  "confirm_worktree_delete": true,
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

/// `agent_defaults.<agent>.<field>` の 1 点だけを user 設定ファイルへ書き込む（ピルの sticky 保存用）。
/// **user ファイル自身の値だけ**を読んで nested に更新する（マージ済み解決値を書き戻すと project 層の値を
/// user へ焼き込んでしまうため）。既存の他 agent・他 field・他キーは保つ。親ディレクトリが無ければ作る。
pub fn persist_agent_default(path: &Path, agent: &str, field: &str, value: &str) -> Result<()> {
    let mut root: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let map = root.as_object_mut().expect("上で object を保証");
    let defaults = map
        .entry("agent_defaults")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !defaults.is_object() {
        *defaults = Value::Object(serde_json::Map::new());
    }
    let agents = defaults.as_object_mut().expect("直前で object を保証");
    let entry = agents
        .entry(agent)
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(serde_json::Map::new());
    }
    entry
        .as_object_mut()
        .expect("直前で object を保証")
        .insert(field.to_string(), Value::String(value.to_string()));
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
    fn persist_agent_default_is_nested_and_isolated() {
        let dir = std::env::temp_dir().join(format!(
            "necoder-agent-defaults-test-{}",
            std::process::id()
        ));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(&path, r#"{ "theme": "necoder-light" }"#).expect("seed");

        // 別 agent・別 field を順に書いても、互いを潰さず nested にマージされる。
        persist_agent_default(&path, "Claude Code", "model", "claude-opus-5").expect("書ける");
        persist_agent_default(&path, "Claude Code", "effort", "xhigh").expect("書ける");
        persist_agent_default(&path, "Codex", "model", "GPT-5.6-Sol").expect("書ける");

        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            &std::fs::read_to_string(&path).expect("read"),
        ])
        .expect("マージできる");
        let defaults = &store.settings().agent_defaults;
        assert_eq!(
            defaults["Claude Code"].model.as_deref(),
            Some("claude-opus-5")
        );
        assert_eq!(defaults["Claude Code"].effort.as_deref(), Some("xhigh"));
        assert_eq!(defaults["Codex"].model.as_deref(), Some("GPT-5.6-Sol"));
        assert_eq!(defaults["Codex"].effort, None); // 書いていない field は None
        assert_eq!(store.settings().theme, "necoder-light"); // 無関係キーは保たれる
        let _ = std::fs::remove_dir_all(&dir);
    }
}
