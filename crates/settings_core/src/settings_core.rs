//! settings_core — 設定の3層マージ（default → user → project）。GPUI 非依存・テスト可能。
//!
//! ARCHITECTURE §7: user = `~/Library/Application Support/Shirushi/settings.json`、
//! project = `.shirushi/settings.json`。後ろのレイヤが前を**深く**上書きする（オブジェクトは再帰マージ、
//! スカラ・配列は置換）。マージ後の JSON を [`Settings`] にデシリアライズする（欠けたキーは型の既定）。

use anyhow::{Context as _, Result};
use serde::Deserialize;
use serde_json::Value;
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
/// 例: `.shirushi/settings.json` に `{ "rail": { "terminal": false } }` でターミナルアイコンを隠す。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct RailSettings {
    pub explorer: bool,
    pub search: bool,
    pub git: bool,
    pub agent: bool,
    pub terminal: bool,
    /// Todo ボード（.shirushi/todos.md・M12-10）。
    pub todos: bool,
    /// リモート SSH（~/.ssh/config のホストへ接続・#2）。
    pub remote: bool,
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
        }
    }
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
    /// 新規スレッドの既定 AI エージェント（表示名。`acp_client::AGENT_LABELS` のいずれか）。
    /// **変更は Settings 画面（★ 既定にする）でのみ** — composer のピルはこのグローバル既定を書き換えない
    /// （哲学「自分で決めた既定はドリフトしない」・DECISIONS §8）。
    pub default_agent: String,
    /// レールのアイコン表示（アクティビティバー）。
    pub rail: RailSettings,
    /// 初回オンボーディングを済ませたか（`false`＝初回で設定ホームが自動オープン・M12）。
    /// 「これで始める」で `true` に。以後は自動では開かない（レール ⚙ からいつでも開ける）。
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "shirushi-dark".to_string(),
            density: Density::Compact,
            font_size: 13.0,
            tab_size: 4,
            soft_wrap: false,
            format_on_save: false,
            locale: None,
            submit_on_enter: false,
            agent_auto_name: true,
            completion_sound: true,
            default_agent: "Claude Code".to_string(),
            rail: RailSettings::default(),
            onboarded: false,
        }
    }
}

/// 組み込みの既定設定（最下層。ユーザーが見られる正の既定値）。
pub const DEFAULT_SETTINGS_JSON: &str = r#"{
  "theme": "shirushi-dark",
  "density": "compact",
  "font_size": 13.0,
  "tab_size": 4,
  "submit_on_enter": false,
  "agent_auto_name": true,
  "completion_sound": true,
  "default_agent": "Claude Code",
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

    /// 既定 + user（任意）+ project（`.shirushi/settings.json`、任意）を読み込む。
    /// 読めないファイルはスキップ、JSON 破損時は既定で継続（黙って落とさず標準エラーに残す）。
    pub fn load(user_path: Option<&Path>, project_dir: Option<&Path>) -> SettingsStore {
        let mut layers: Vec<String> = vec![DEFAULT_SETTINGS_JSON.to_string()];
        if let Some(path) = user_path {
            if let Ok(text) = std::fs::read_to_string(path) {
                layers.push(text);
            }
        }
        if let Some(dir) = project_dir {
            let path = dir.join(".shirushi").join("settings.json");
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

/// user 設定ファイルの標準パス（macOS）。`HOME` が無ければ `None`。
pub fn user_settings_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/Shirushi/settings.json"))
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
        assert_eq!(store.settings().theme, "shirushi-dark");
        assert_eq!(store.settings().density, Density::Compact);
        assert_eq!(store.settings().tab_size, 4);
    }

    #[test]
    fn user_layer_overrides_default() {
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "theme": "shirushi-light", "tab_size": 2 }"#,
        ])
        .expect("マージできる");
        assert_eq!(store.settings().theme, "shirushi-light");
        assert_eq!(store.settings().tab_size, 2);
        // 触れていないキーは既定のまま
        assert_eq!(store.settings().density, Density::Compact);
    }

    #[test]
    fn project_layer_overrides_user() {
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            r#"{ "theme": "shirushi-light", "density": "cozy" }"#, // user
            r#"{ "theme": "shirushi-dark" }"#,                     // project が最優先
        ])
        .expect("マージできる");
        assert_eq!(store.settings().theme, "shirushi-dark"); // project 勝ち
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
    fn persist_user_value_sets_one_key_and_keeps_others() {
        let dir =
            std::env::temp_dir().join(format!("shirushi-settings-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        // 既存にユーザー値がある状態を作る
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(&path, r#"{ "theme": "shirushi-light" }"#).expect("seed");

        persist_user_value(&path, "submit_on_enter", Value::Bool(true)).expect("書ける");
        let store = SettingsStore::from_json_layers(&[
            DEFAULT_SETTINGS_JSON,
            &std::fs::read_to_string(&path).expect("read"),
        ])
        .expect("マージできる");
        assert!(store.settings().submit_on_enter); // 書いたキー
        assert_eq!(store.settings().theme, "shirushi-light"); // 既存キーは保たれる
        let _ = std::fs::remove_dir_all(&dir);
    }
}
