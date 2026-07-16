//! keymap_core — JSON keymap をコンテキスト述語つきで gpui の [`KeyBinding`] に変換する。
//!
//! 形式（Zed 互換の縮小版）:
//! ```json
//! [
//!   { "context": "Editor", "bindings": { "cmd-s": "editor::Save", "enter": "editor::Newline" } }
//! ]
//! ```
//! アクションは名前（`namespace::Name`）で参照し、実行時に [`App::build_action`] で解決する
//! （＝この crate は具体アクション型に依存しない）。未登録アクション・不正述語・不正キーは
//! **その項目だけ skip** して警告し、残りは活かす（部分ロード）。

use anyhow::{Context as _, Result};
use gpui::{App, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::rc::Rc;

/// keymap の 1 セクション（1 つのコンテキスト述語に対する束）。
#[derive(Debug, Clone, Deserialize)]
pub struct KeymapSection {
    /// コンテキスト述語（例 `"Editor"`, `"Workspace && !menu"`）。空なら全域。
    #[serde(default)]
    pub context: String,
    /// キーストローク → アクション名。
    #[serde(default)]
    pub bindings: BTreeMap<String, String>,
}

/// JSON をセクション列にパースする（構造の検証のみ・アクション解決はしない）。
pub fn parse(json: &str) -> Result<Vec<KeymapSection>> {
    serde_json::from_str(json).context("keymap JSON の解析に失敗")
}

/// JSON keymap を [`KeyBinding`] 列へ変換する。壊れた項目は skip して警告（部分ロード）。
pub fn load_bindings(json: &str, cx: &App) -> Result<Vec<KeyBinding>> {
    let sections = parse(json)?;
    let mapper = cx.keyboard_mapper().clone();
    let mut bindings = Vec::new();

    for section in sections {
        let predicate: Option<Rc<KeyBindingContextPredicate>> = if section.context.trim().is_empty()
        {
            None
        } else {
            match KeyBindingContextPredicate::parse(&section.context) {
                Ok(predicate) => Some(Rc::new(predicate)),
                Err(error) => {
                    eprintln!("keymap: context `{}` の述語が不正: {error}", section.context);
                    continue;
                }
            }
        };

        for (keystrokes, action_name) in &section.bindings {
            let action = match cx.build_action(action_name, None) {
                Ok(action) => action,
                Err(error) => {
                    eprintln!("keymap: アクション `{action_name}` を解決できない: {error}");
                    continue;
                }
            };
            match KeyBinding::load(
                keystrokes,
                action,
                predicate.clone(),
                false,
                None,
                mapper.as_ref(),
            ) {
                Ok(binding) => bindings.push(binding),
                Err(error) => eprintln!("keymap: キー `{keystrokes}` が不正: {error}"),
            }
        }
    }
    Ok(bindings)
}

/// 組み込み既定 keymap（macOS・Zed 互換ベース）。編集アクションは `editor` 名前空間、終了は `shirushi`。
pub const DEFAULT_KEYMAP_JSON: &str = r#"[
  {
    "context": "Editor",
    "bindings": {
      "backspace": "editor::Backspace",
      "delete": "editor::Delete",
      "enter": "editor::Newline",
      "shift-enter": "editor::InsertNewline",
      "left": "editor::MoveLeft",
      "right": "editor::MoveRight",
      "up": "editor::MoveUp",
      "down": "editor::MoveDown",
      "home": "editor::MoveToLineStart",
      "end": "editor::MoveToLineEnd",
      "cmd-left": "editor::MoveToLineStart",
      "cmd-right": "editor::MoveToLineEnd",
      "shift-left": "editor::SelectLeft",
      "shift-right": "editor::SelectRight",
      "shift-up": "editor::SelectUp",
      "shift-down": "editor::SelectDown",
      "cmd-a": "editor::SelectAll",
      "cmd-c": "editor::Copy",
      "cmd-x": "editor::Cut",
      "cmd-v": "editor::Paste",
      "cmd-z": "editor::Undo",
      "cmd-shift-z": "editor::Redo",
      "cmd-s": "editor::Save",
      "cmd-enter": "agent::SubmitPrompt",
      "f12": "workspace::GoToDefinition",
      "ctrl-space": "workspace::TriggerCompletion"
    }
  },
  {
    "context": "AgentPanel",
    "bindings": {
      "cmd-w": "agent::CloseActiveThread"
    }
  },
  {
    "bindings": {
      "cmd-p": "workspace::FileFinder",
      "cmd-o": "workspace::ProjectSwitcher",
      "cmd-shift-f": "workspace::ProjectSearch",
      "cmd-shift-t": "workspace::RestoreClosedTab",
      "cmd-k cmd-t": "workspace::ThemeSelector",
      "cmd-j": "workspace::ToggleTerminal",
      "cmd-\\": "workspace::SplitRight",
      "ctrl-shift-g": "workspace::ToggleGitPanel",
      "cmd-w": "workspace::CloseTab",
      "cmd-}": "workspace::SelectNextTab",
      "cmd-{": "workspace::SelectPrevTab",
      "cmd-shift-a": "workspace::NewThread",
      "cmd-alt-right": "workspace::SelectNextThread",
      "cmd-alt-left": "workspace::SelectPrevThread",
      "ctrl-tab": "workspace::SelectNextThread",
      "ctrl-shift-tab": "workspace::SelectPrevThread",
      "cmd-shift-n": "workspace::NewWindow",
      "cmd-1": "workspace::ActivateProject1",
      "cmd-2": "workspace::ActivateProject2",
      "cmd-3": "workspace::ActivateProject3",
      "cmd-4": "workspace::ActivateProject4",
      "cmd-5": "workspace::ActivateProject5",
      "cmd-6": "workspace::ActivateProject6",
      "cmd-7": "workspace::ActivateProject7",
      "cmd-8": "workspace::ActivateProject8",
      "cmd-9": "workspace::ActivateProject9",
      "cmd-q": "shirushi::Quit"
    }
  }
]"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_bindings() {
        let sections = parse(DEFAULT_KEYMAP_JSON).expect("既定 keymap がパースできる");
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].context, "Editor");
        assert_eq!(
            sections[0].bindings.get("cmd-s").map(String::as_str),
            Some("editor::Save")
        );
        // 2 セクション目は AgentPanel（⌘W でアクティブスレッドを閉じる）
        assert_eq!(sections[1].context, "AgentPanel");
        assert_eq!(
            sections[1].bindings.get("cmd-w").map(String::as_str),
            Some("agent::CloseActiveThread")
        );
        // 3 セクション目は全域（context 空）+ Quit
        assert!(sections[2].context.is_empty());
        assert_eq!(
            sections[2].bindings.get("cmd-q").map(String::as_str),
            Some("shirushi::Quit")
        );
    }

    #[test]
    fn missing_fields_default_gracefully() {
        // context 省略・bindings 省略でも壊れない
        let sections = parse(r#"[{ "bindings": { "esc": "app::Escape" } }, {}]"#).unwrap();
        assert_eq!(sections.len(), 2);
        assert!(sections[0].context.is_empty());
        assert!(sections[1].bindings.is_empty());
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse("not json").is_err());
    }
}
